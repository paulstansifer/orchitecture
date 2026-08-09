//! Headless testing mode: a line-oriented stdin/stdout REPL over a real Bevy
//! `App` (via `MinimalPlugins` -- no window, renderer, or GPU), driving the
//! game's actual resources and change-detection-gated systems. Meant to give
//! scripted (e.g. LLM-driven) test scenarios a fast way to exercise city
//! construction, farms, time advancement, and change-detection reactions
//! (nav-grid rebuilds, home syncing) without a GUI.
//!
//! ## Protocol
//! One command per input line (whitespace-separated tokens). Each command
//! produces a response: the first line is `OK` (optionally followed by more
//! detail) or `ERR <message>`, any further detail lines follow, and a blank
//! line marks the end of the response. Run `help` for the command list.
//!
//! ## Change detection
//! Mutating commands (`place`, `advance`, ...) only mutate resources -- they
//! do *not* advance the Bevy schedule. Call `tick` to run one `Update` pass,
//! which is when change-detection-gated systems (`rebuild_navigation_grid`,
//! `sync_assignments`) actually react, and `query changed` reports what reacted on
//! the most recent `tick`. This lets a test script mutate, tick, and observe
//! exactly which systems fired -- and confirm a second `tick` with no
//! intervening mutation causes nothing to fire again.

use std::io::{self, BufRead, Write};

use bevy::app::{App, Update};
use bevy::ecs::system::RunSystemOnce;
use bevy::math::IVec3;
use bevy::prelude::*;
use rand::{rngs::StdRng, SeedableRng};

use crate::build_helpers::Builder;
use crate::city::{get_real_and_proposed, Cell, ConstructedCity, Proposal, ProposedCity};
use crate::construction;
use crate::eorf::{load_structure_info, EorfId};
use crate::evaluation::compute_outdoorsness;
use crate::materials::{BuildMaterialId, MaterialList};
use crate::pathing::NavigationGrid;
use crate::place;
use crate::population::{assign_places, Population};
use crate::resource::{ToolKind, UniformResource};
use crate::serialization;
use crate::simulation::{SimulationPlugin, SimulationSystems};
use crate::sparse3d::{Facing, Slot, SlotCoord};
use crate::surroundings::farmstead::{
    farm_breakdown, FarmEvent, FarmId, FarmProduction, FarmsResource, GameClock, NewProduction,
};
use crate::traveler::{setup_travelers, TravelerState};

const HELP_TEXT: &str = "\
Commands (space-separated tokens, one per line):
  place <x> <y> <z> <slot> <structure> [facing]    propose (or, in sandbox, build) a structure
  remove <x> <y> <z> <slot>                        propose (or, in sandbox, commit) a removal
  build_box <x1> <y1> <z1> <x2> <y2> <z2>          build a hollow box (walls/floor/ceiling)
  construct                                        commit all pending proposals now
  undo / redo                                      undo/redo the last proposal edit
  sandbox on|off                                   toggle direct-build vs propose-then-construct
  advance [n]                                      advance the game clock by n months (default 1)
  tick                                             run one Bevy Update pass (change detection!)
  invite <farm_idx> / uninvite <farm_idx>          toggle a farm's market invitation
  invite_traveler / uninvite_traveler              accept or decline this month's traveler offer
  farm_event <farm_idx> market|reroll|specialize|adopt   set a farm's next market action
  set_production <farm_idx> <resource>             cheat: force a farm's produced resource
  set_inventory <resource> <qty>                   cheat: adjust city inventory of a resource(if possible)
  deposit_tool                                     cheat: deposit a carpenter's tools into rack storage
  learn <idea> <count>                             cheat: learn an idea's first <count> segments
  set_priority <x> <y> <z> <level>                 set a workplace's priority (very_low|low|medium|high|very_high)
  install <x> <y> <z> <slot_idx>                   install the first available matching resource into a slot
  uninstall <x> <y> <z> <slot_idx>                 remove an installed resource, returning it to storage
  query cell <x> <y> <z> <slot>                    inspect a grid location
  query structures                                 list placeable structures
  query places                                     list place types (Places form automatically)
  query slots <x> <y> <z>                          list a furniture's slots and their contents
  query place <x> <y> <z>                          inspect the place owning a cube
  query valid_places <x> <y> <z>                   place types formable around a cube
  query ideas                                      per-idea learned/understood segments, and idea-gated places
  query population                                 each individual's home/work/fed/morale
  query farms                                      list all farms
  query farm <idx>                                 detailed farm info + market/production preview
  query outdoorness <x> <y> <z>                    outdoorsness (0.0-1.0) at a cube
  query month                                      current game-clock month
  query traveler                                   current traveler offer's origin/path, if any
  query inventory                                  total stored resources across all places
  query proposals                                  pending-proposal count / construction timer
  query changed                                    what reacted on the most recent tick
  query path <x1> <y1> <z1> <x2> <y2> <z2>         route between two Room cells (needs a tick)
  query connected <x1> <y1> <z1> <x2> <y2> <z2>    cheap reachability check (needs a tick)
  dump                                             print the city as the on-disk text format
  save <path> / load <path>                        save/load the city to/from a text file
  reset [seed]                                     start a brand-new session
  help                                             this text
  quit / exit                                      end the session
Slots: room|floor|xwall|zwall. Facings: negx|negz|posx|posz (or 0-3). Eorf names use
underscores for spaces (e.g. market_stand). Pathfinding queries read `NavigationGrid`, which
is only rebuilt by a `tick` after the city changes.";

/// The headless session's own RNG, threaded through the initial farm layout,
/// the initial place's bin placement, and `advance`'s market/production/
/// traveler rolls -- so a session's *entire* random behavior is deterministic
/// given `--seed` (see `HeadlessSession::new`, which seeds farm generation
/// from this same stream before it's stashed as a resource). Traveler configs
/// come from a static `.ron` file and involve no randomness of their own.
#[derive(Resource)]
struct HeadlessRng(StdRng);

#[derive(Resource, Clone, Copy)]
struct SandboxFlag(bool);

/// What changed on the most recent `tick`, as observed by `report_changes_system`
/// (a persistent system, so its `is_changed()` reads are meaningful -- unlike a
/// freshly-constructed one-off system, which would always report "changed").
#[derive(Resource, Default, Clone, Copy)]
struct ChangeReport {
    constructed_city: bool,
    proposed_city: bool,
    population: bool,
    farms: bool,
    nav_grid: bool,
    ideas: bool,
}

fn report_changes_system(
    mut report: ResMut<ChangeReport>,
    constructed: Res<ConstructedCity>,
    pending: Res<ProposedCity>,
    population: Res<Population>,
    farms: Res<FarmsResource>,
    nav_grid: Option<Res<NavigationGrid>>,
    idea_state: Res<crate::idea::IdeaState>,
) {
    report.constructed_city = constructed.is_changed();
    report.proposed_city = pending.is_changed();
    report.population = population.is_changed();
    report.farms = farms.is_changed();
    report.nav_grid = nav_grid.is_some_and(|g| g.is_changed());
    report.ideas = idea_state.is_changed();
}

pub struct HeadlessSession {
    app: App,
}

impl HeadlessSession {
    pub fn new(seed: u64) -> Self {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let mut rng = StdRng::seed_from_u64(seed);
        let structures = load_structure_info();
        let mut constructed = ConstructedCity::new(structures);
        constructed.places = place::load_place_info(&constructed.eorfs, &constructed.ideas);
        place::place_initial_places(&mut constructed);

        // Draws from the same seeded stream as everything else below, so the
        // initial farm layout is deterministic given `--seed` too.
        let farms = crate::surroundings::map::build_farms_resource(&mut rng);

        app.insert_resource(constructed);
        app.insert_resource(ProposedCity::new());
        app.insert_resource(Population::default());
        app.insert_resource(crate::idea::IdeaState::default());
        app.insert_resource(GameClock::default());
        app.insert_resource(MaterialList::load());
        app.insert_resource(farms);
        app.insert_resource(HeadlessRng(rng));
        app.insert_resource(SandboxFlag(true));
        app.insert_resource(ChangeReport::default());

        // Only needs `Commands` and involves no randomness, so just run the
        // real startup system directly rather than duplicating its logic.
        app.world_mut()
            .run_system_once(setup_travelers)
            .expect("setup_travelers");

        // The same change-gated systems the game runs, from the same place --
        // see `simulation.rs` for why this must not be re-declared here.
        app.add_plugins(SimulationPlugin);
        app.add_systems(Update, report_changes_system.after(SimulationSystems));

        // Settle the initial world (nav grid, home assignment) before the first
        // user-issued `tick`, mirroring the game's first frame after Startup.
        app.update();

        HeadlessSession { app }
    }

    fn world(&mut self) -> &mut World {
        self.app.world_mut()
    }

    fn find_structure(&mut self, name: &str) -> Option<EorfId> {
        let name = name.replace('_', " ");
        self.world()
            .resource::<ConstructedCity>()
            .find_structure_by_name(&name)
    }

    fn describe_cell(cw: &ConstructedCity, ml: &MaterialList, cell: &Cell) -> String {
        let info = &cw.eorfs[cell.id.as_usize()];
        let mut s = format!(
            "{} facing={:?} material={}",
            info.name.replace(' ', "_"),
            cell.facing,
            cell.material(&cw.eorfs, ml).label()
        );
        if let Some(eval) = &cell.evaluation {
            s.push_str(&format!(
                " evaluation(order={:?}, interest={:?})",
                eval.order, eval.interest
            ));
        }
        s
    }

    /// If sandbox mode is on, immediately commit any pending proposals.
    fn maybe_construct(&mut self) {
        if self.world().resource::<SandboxFlag>().0 {
            let n = self.world().run_system_once(construct_system).unwrap_or(0);
            let _ = n;
        }
    }

    pub fn dispatch(&mut self, line: &str) -> Result<Vec<String>, String> {
        let args: Vec<&str> = line.split_whitespace().collect();
        let Some((&cmd, args)) = args.split_first() else {
            return Ok(vec![]);
        };

        match cmd {
            "help" => Ok(HELP_TEXT.lines().map(str::to_string).collect()),
            "tick" => self.cmd_tick(),
            "sandbox" => self.cmd_sandbox(args),
            "place" => self.cmd_place(args),
            "remove" => self.cmd_remove(args),
            "build_box" => self.cmd_build_box(args),
            "construct" => self.cmd_construct(),
            "undo" => self.cmd_undo(),
            "redo" => self.cmd_redo(),
            "advance" => self.cmd_advance(args),
            "invite" | "uninvite" => self.cmd_invite(cmd, args),
            // The graphical UI sets this from the traveler panel's checkbox;
            // `advance` reads it when resolving the visit.
            "invite_traveler" | "uninvite_traveler" => self.cmd_invite_traveler(cmd),
            "farm_event" => self.cmd_farm_event(args),
            // Test-only cheat: force a farm's production, bypassing the normal
            // Reconfigure-and-pay-cost flow (see `farm_event ... reroll`).
            "set_production" => self.cmd_set_production(args),
            "set_inventory" => self.cmd_set_inventory(args),
            "set_priority" => self.cmd_set_priority(args),
            "deposit_tool" => self.cmd_deposit_tool(),
            // A book's worth of segments, but deterministic: tests need to
            // land on an exact progress fraction (e.g. 25/50 == the
            // carpenter's workshop's unlock threshold), which a random roll
            // can't promise.
            "learn" => self.cmd_learn(args),
            "install" => self.cmd_install(args),
            "uninstall" => self.cmd_uninstall(args),
            "query" => self.query(args),
            "dump" => self.cmd_dump(),
            "save" => self.cmd_save(args),
            "load" => self.cmd_load(args),
            "reset" => self.cmd_reset(args),
            other => Err(format!("unknown command: {other} (try 'help')")),
        }
    }

    fn cmd_tick(&mut self) -> Result<Vec<String>, String> {
        self.app.update();
        let report = *self.world().resource::<ChangeReport>();
        Ok(vec![format_change_report(&report)])
    }

    fn cmd_sandbox(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let want = match args.first().copied() {
            Some("on") => true,
            Some("off") => false,
            _ => return Err("usage: sandbox on|off".to_string()),
        };
        let was = self.world().resource::<SandboxFlag>().0;
        self.world().resource_mut::<SandboxFlag>().0 = want;
        if want && !was {
            self.maybe_construct();
        }
        Ok(vec![format!("sandbox={want}")])
    }

    fn cmd_place(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 5 {
            return Err("usage: place <x> <y> <z> <slot> <structure> [facing]".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let slot = parse_slot(args[3])?;
        let id = self
            .find_structure(args[4])
            .ok_or_else(|| format!("unknown structure: {}", args[4]))?;
        let dir = args
            .get(5)
            .map(|f| parse_facing(f))
            .transpose()?
            .unwrap_or(0);
        let outcome = self
            .world()
            .run_system_once_with(place_system, (SlotCoord { cube, slot }, Some(id), dir))
            .map_err(|e| e.to_string())?;
        self.maybe_construct();
        Ok(place_outcome_lines(outcome))
    }

    fn cmd_remove(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 4 {
            return Err("usage: remove <x> <y> <z> <slot>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let slot = parse_slot(args[3])?;
        let outcome = self
            .world()
            .run_system_once_with(place_system, (SlotCoord { cube, slot }, None, 0))
            .map_err(|e| e.to_string())?;
        self.maybe_construct();
        Ok(place_outcome_lines(outcome))
    }

    fn cmd_build_box(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 6 {
            return Err("usage: build_box <x1> <y1> <z1> <x2> <y2> <z2>".to_string());
        }
        let a = parse_ivec3(&args[0..3])?;
        let b = parse_ivec3(&args[3..6])?;
        let n = self
            .world()
            .run_system_once_with(build_box_system, (a, b))
            .map_err(|e| e.to_string())?;
        self.maybe_construct();
        Ok(vec![format!("changed={n}")])
    }

    fn cmd_construct(&mut self) -> Result<Vec<String>, String> {
        let n = self
            .world()
            .run_system_once(construct_system)
            .map_err(|e| e.to_string())?;
        Ok(vec![format!("committed={n}")])
    }

    fn cmd_undo(&mut self) -> Result<Vec<String>, String> {
        let n = self
            .world()
            .run_system_once(undo_system)
            .map_err(|e| e.to_string())?;
        self.maybe_construct();
        Ok(vec![format!("changed={n}")])
    }

    fn cmd_redo(&mut self) -> Result<Vec<String>, String> {
        let n = self
            .world()
            .run_system_once(redo_system)
            .map_err(|e| e.to_string())?;
        self.maybe_construct();
        Ok(vec![format!("changed={n}")])
    }

    fn cmd_advance(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let n: u32 = args
            .first()
            .map(|s| s.parse().map_err(|_| format!("not an integer: {s}")))
            .transpose()?
            .unwrap_or(1);
        let mut lines = Vec::new();
        for _ in 0..n {
            lines.extend(
                self.world()
                    .run_system_once(advance_month_system)
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(lines)
    }

    fn cmd_invite(&mut self, cmd: &str, args: &[&str]) -> Result<Vec<String>, String> {
        let idx = parse_usize(args.first().copied().unwrap_or(""))?;
        let invited = cmd == "invite";
        let farms = &mut self.world().resource_mut::<FarmsResource>();
        let farm = farms
            .farms
            .get_mut(idx)
            .ok_or_else(|| format!("no such farm: {idx}"))?;
        farm.invited = invited;
        Ok(vec![format!("farm {idx} invited={invited}")])
    }

    fn cmd_invite_traveler(&mut self, cmd: &str) -> Result<Vec<String>, String> {
        let invited = cmd == "invite_traveler";
        let mut traveler_state = self.world().resource_mut::<TravelerState>();
        if traveler_state.current_offer.is_none() {
            return Err("no traveler offer this month".to_string());
        }
        traveler_state.invited = invited;
        Ok(vec![format!("traveler invited={invited}")])
    }

    fn cmd_farm_event(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let idx = parse_usize(args.first().copied().unwrap_or(""))?;
        let event = match args.get(1).copied() {
            Some("market") => FarmEvent::Market,
            Some("reroll") => FarmEvent::Reconfigure(NewProduction::RandomRegular),
            Some("specialize") => {
                FarmEvent::Reconfigure(NewProduction::Tool(ToolKind::CarpentersTools))
            }
            Some("adopt") => FarmEvent::Adopt,
            _ => return Err("usage: farm_event <idx> market|reroll|specialize|adopt".to_string()),
        };
        let mut farms = self.world().resource_mut::<FarmsResource>();
        if idx >= farms.farms.len() {
            return Err(format!("no such farm: {idx}"));
        }
        farms.ensure_adjacency();
        farms.set_farm_event(FarmId::new(idx), event);
        Ok(vec!["ok".to_string()])
    }

    fn cmd_set_production(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let idx = parse_usize(args.first().copied().unwrap_or(""))?;
        let resource = args
            .get(1)
            .copied()
            .ok_or_else(|| "usage: set_production <idx> <resource>".to_string())
            .and_then(parse_uniform_resource)?;
        let mut farms = self.world().resource_mut::<FarmsResource>();
        let farm = farms
            .farms
            .get_mut(idx)
            .ok_or_else(|| format!("no such farm: {idx}"))?;
        farm.production = FarmProduction::Regular(resource);
        Ok(vec![format!("farm {idx} produces={}", resource.label())])
    }

    fn cmd_set_inventory(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let resource = args
            .first()
            .copied()
            .ok_or_else(|| "usage: set_production <idx> <resource>".to_string())
            .and_then(parse_uniform_resource)?;
        let qty = parse_usize(args.get(1).copied().unwrap_or(""))? as u32;
        let mut cw = self.world().resource_mut::<ConstructedCity>();
        let cur_amt = crate::storage::total_uniform(&cw, resource);
        let mut descr = vec![];
        if qty > cur_amt {
            let depositied =
                crate::storage::deposit_uniform_with_capacity(&mut cw, resource, qty - cur_amt);
            descr.push(format!("{depositied} deposited"));
        } else {
            let withdrawn = crate::storage::consume_uniform(&mut cw, resource, cur_amt - qty);
            descr.push(format!("{withdrawn} withdrawn"));
        }
        Ok(descr)
    }

    fn cmd_set_priority(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 4 {
            return Err(
                "usage: set_priority <x> <y> <z> <very_low|low|medium|high|very_high>".to_string(),
            );
        }
        let cube = parse_ivec3(&args[0..3])?;
        let prio = match args[3] {
            "very_low" => crate::work::WorkPriority::VeryLow,
            "low" => crate::work::WorkPriority::Low,
            "medium" => crate::work::WorkPriority::Medium,
            "high" => crate::work::WorkPriority::High,
            "very_high" => crate::work::WorkPriority::VeryHigh,
            other => return Err(format!("unknown priority level {other:?}")),
        };
        let mut cw = self.world().resource_mut::<ConstructedCity>();
        let core = match place::place_id_at(&cw, cube) {
            Some(id) => place::place_location(&cw, id),
            None => return Err("no place there".to_string()),
        };
        cw.work_priorities.insert(core, prio);
        Ok(vec![format!("priority at {core} = {}", prio.label())])
    }

    fn cmd_deposit_tool(&mut self) -> Result<Vec<String>, String> {
        let mut cw = self.world().resource_mut::<ConstructedCity>();
        if crate::storage::deposit_tool(&mut cw, ToolKind::CarpentersTools) {
            Ok(vec!["deposited carpenter's tools".to_string()])
        } else {
            Err("no rack storage with room for a tool".to_string())
        }
    }

    fn cmd_learn(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 2 {
            return Err("usage: learn <idea> <count>".to_string());
        }
        let name = args[0].replace('_', " ");
        let count = parse_usize(args[1])?;
        let idea = {
            let cw = self.world().resource::<ConstructedCity>();
            crate::idea::idea_by_name(&cw.ideas, &name)
                .ok_or_else(|| format!("no such idea: {name}"))?
        };
        let count = count.min(crate::idea::SEGMENTS as usize);
        let mask = if count == 64 {
            u64::MAX
        } else {
            (1u64 << count) - 1
        };
        let mut idea_state = self.world().resource_mut::<crate::idea::IdeaState>();
        idea_state.learn(idea, mask);
        let learned = idea_state.learned[idea].count_ones();
        Ok(vec![format!(
            "{name}: {learned}/{} segments learned",
            crate::idea::SEGMENTS
        )])
    }

    fn cmd_install(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 4 {
            return Err("usage: install <x> <y> <z> <slot_idx>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let slot_idx = parse_usize(args[3])?;
        let loc = SlotCoord {
            cube,
            slot: Slot::Room,
        };
        let mut cw = self.world().resource_mut::<ConstructedCity>();
        let eorf_idx = cw
            .contents
            .get(loc)
            .ok_or_else(|| "no furniture there".to_string())?
            .id
            .as_usize();
        let slots = cw.eorfs[eorf_idx].slots.clone();
        let slot = slots
            .get(slot_idx)
            .ok_or_else(|| format!("no slot {slot_idx} on this furniture"))?;
        if cw.slot_contents(cube, slot_idx).is_some() {
            return Err("slot already filled".to_string());
        }
        let kind = slot.kind;
        let item = crate::storage::available_uniques_of_kind(&cw, kind)
            .into_iter()
            .next()
            .ok_or_else(|| format!("no {} available in storage", kind.label()))?;
        crate::storage::withdraw_unique(&mut cw, &item);
        let label = item.label();
        cw.set_slot(cube, slot_idx, slots.len(), Some(item));
        Ok(vec![format!("installed {label}")])
    }

    fn cmd_uninstall(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 4 {
            return Err("usage: uninstall <x> <y> <z> <slot_idx>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let slot_idx = parse_usize(args[3])?;
        let loc = SlotCoord {
            cube,
            slot: Slot::Room,
        };
        let mut cw = self.world().resource_mut::<ConstructedCity>();
        let eorf_idx = cw
            .contents
            .get(loc)
            .ok_or_else(|| "no furniture there".to_string())?
            .id
            .as_usize();
        let slot_count = cw.eorfs[eorf_idx].slots.len();
        let item = cw
            .slot_contents(cube, slot_idx)
            .cloned()
            .ok_or_else(|| "slot is empty".to_string())?;
        cw.set_slot(cube, slot_idx, slot_count, None);
        let deposited = crate::storage::deposit_unique(&mut cw, item.clone());
        Ok(vec![format!(
            "removed {}{}",
            item.label(),
            if deposited {
                ""
            } else {
                " (no storage room -- dropped)"
            }
        )])
    }

    fn cmd_dump(&mut self) -> Result<Vec<String>, String> {
        let cw = self.world().resource::<ConstructedCity>();
        let bytes = serialization::serialize(&cw.contents, &cw.eorfs).map_err(|e| e.to_string())?;
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        Ok(text.lines().map(str::to_string).collect())
    }

    fn cmd_save(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let path = args.first().ok_or("usage: save <path>")?;
        let cw = self.world().resource::<ConstructedCity>();
        serialization::save(&cw.contents, &cw.eorfs, &std::path::PathBuf::from(path))
            .map_err(|e| e.to_string())?;
        Ok(vec![format!("saved to {path}")])
    }

    fn cmd_load(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let path = args.first().ok_or("usage: load <path>")?;
        if !std::path::Path::new(path).exists() {
            return Err(format!("no such file: {path}"));
        }
        let new_contents = {
            let cw = self.world().resource::<ConstructedCity>();
            serialization::load(&std::path::PathBuf::from(path), &cw.eorfs)
                .map_err(|e| e.to_string())?
        };
        self.world()
            .run_system_once_with(load_system, new_contents)
            .map_err(|e| e.to_string())?;
        Ok(vec![format!("loaded {path}")])
    }

    fn cmd_reset(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let seed = args
            .first()
            .map(|s| s.parse().map_err(|_| format!("not an integer: {s}")))
            .transpose()?
            .unwrap_or_else(rand::random);
        *self = HeadlessSession::new(seed);
        Ok(vec![format!("seed={seed}")])
    }

    fn query(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let Some((&sub, args)) = args.split_first() else {
            return Err(
                "usage: query <cell|structures|places|place|valid_places|ideas|\
                 population|farms|farm|roads|outdoorness|month|traveler|inventory|proposals|\
                 changed|path|connected> ..."
                    .to_string(),
            );
        };
        match sub {
            "cell" => self.query_cell(args),
            "slots" => self.query_slots(args),
            "structures" => self.query_structures(),
            "places" => self.query_places(),
            "place" => self.query_place(args),
            "valid_places" => self.query_valid_places(args),
            "ideas" => self.query_ideas(),
            "population" => self.query_population(),
            "farms" => self.query_farms(),
            "farm" => self.query_farm(args),
            "roads" => self.query_roads(),
            "outdoorness" => self.query_outdoorness(args),
            "month" => self.query_month(),
            "traveler" => self.query_traveler(),
            "inventory" => self.query_inventory(),
            "proposals" => self.query_proposals(),
            "changed" => self.query_changed(),
            "path" => self.query_path(args),
            "connected" => self.query_connected(args),
            other => Err(format!("unknown query: {other}")),
        }
    }

    fn query_cell(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 4 {
            return Err("usage: query cell <x> <y> <z> <slot>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let slot = parse_slot(args[3])?;
        let loc = SlotCoord { cube, slot };
        let world = self.world();
        let cw = world.resource::<ConstructedCity>();
        let ml = world.resource::<MaterialList>();
        let pending = world.resource::<ProposedCity>();
        let (real, _) = get_real_and_proposed(cw, pending, loc);
        let real_desc = real
            .map(|c| Self::describe_cell(cw, ml, c))
            .unwrap_or_else(|| "empty".to_string());
        let proposal_desc = match pending.proposed_changes.get(loc) {
            Some(Proposal::Remove) => "remove".to_string(),
            Some(Proposal::Place(cell)) if real.is_some() => {
                format!("replace with {}", Self::describe_cell(cw, ml, cell))
            }
            Some(Proposal::Place(cell)) => {
                format!("add {}", Self::describe_cell(cw, ml, cell))
            }
            None => "none".to_string(),
        };
        Ok(vec![
            format!("real: {real_desc}"),
            format!("proposed: {proposal_desc}"),
        ])
    }

    fn query_slots(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 3 {
            return Err("usage: query slots <x> <y> <z>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let loc = SlotCoord {
            cube,
            slot: Slot::Room,
        };
        let cw = self.world().resource::<ConstructedCity>();
        let eorf_idx = cw
            .contents
            .get(loc)
            .ok_or_else(|| "no furniture there".to_string())?
            .id
            .as_usize();
        let slots = &cw.eorfs[eorf_idx].slots;
        if slots.is_empty() {
            return Ok(vec!["no slots".to_string()]);
        }
        Ok(slots
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                let contents = cw
                    .slot_contents(cube, i)
                    .map(|item| item.label())
                    .unwrap_or_else(|| "empty".to_string());
                format!("{i} {}: {contents}", slot.kind.label())
            })
            .collect())
    }

    fn query_structures(&mut self) -> Result<Vec<String>, String> {
        let cw = self.world().resource::<ConstructedCity>();
        Ok(cw
            .eorfs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    "{i} {} style={:?} type={:?} furniture={}",
                    s.name.replace(' ', "_"),
                    s.placement_style,
                    s.element_type(),
                    s.is_furniture()
                )
            })
            .collect())
    }

    fn query_places(&mut self) -> Result<Vec<String>, String> {
        let cw = self.world().resource::<ConstructedCity>();
        Ok(cw
            .places
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let reqs = s
                    .requirements
                    .iter()
                    .map(|r| {
                        format!(
                            "{}x{}..{:?}",
                            r.min,
                            r.requirement.name().replace(' ', "_"),
                            r.max
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "{i} {} requires=[{reqs}] storage={}",
                    s.name.replace(' ', "_"),
                    s.public_storage
                )
            })
            .collect())
    }

    fn query_place(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 3 {
            return Err("usage: query place <x> <y> <z>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let cw = self.world().resource::<ConstructedCity>();
        match place::place_id_at(cw, cube) {
            None => Ok(vec!["none".to_string()]),
            Some(id) => {
                let ps = &cw.placed_places[id];
                let info = &cw.places[ps.place];
                let mut lines = vec![format!(
                    "id={id} type={} structures={}",
                    info.name.replace(' ', "_"),
                    ps.fulfillments.len()
                )];
                for (res, qty) in ps.contents.uniform_totals() {
                    lines.push(format!("  {}: {qty}", res.label()));
                }
                let tools = ps.contents.tool_count();
                if tools > 0 {
                    lines.push(format!("  tools: {tools}"));
                }
                Ok(lines)
            }
        }
    }

    fn query_valid_places(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 3 {
            return Err("usage: query valid_places <x> <y> <z>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let cw = self.world().resource::<ConstructedCity>();
        let ids = place::valid_places_for(cw, cube);
        if ids.is_empty() {
            Ok(vec!["(none)".to_string()])
        } else {
            Ok(ids
                .into_iter()
                .map(|i| format!("{i} {}", cw.places[i].name.replace(' ', "_")))
                .collect())
        }
    }

    fn query_ideas(&mut self) -> Result<Vec<String>, String> {
        let idea_state = self.world().resource::<crate::idea::IdeaState>().clone();
        let cw = self.world().resource::<ConstructedCity>();
        let segments = crate::idea::SEGMENTS;
        let mut lines: Vec<String> = cw
            .ideas
            .iter()
            .enumerate()
            .map(|(i, idea)| {
                let understood = cw.understood[i].count_ones();
                let pending = (idea_state.learned[i] & !cw.understood[i]).count_ones();
                format!(
                    "{i} {} understood={understood}/{segments} pending={pending}",
                    idea.name.replace(' ', "_")
                )
            })
            .collect();
        // Gated place kinds and where they currently stand, since that's
        // the point of tracking ideas at all.
        for (i, def) in cw.places.iter().enumerate() {
            let Some(gate) = &def.gate else { continue };
            let status = match place::gate_efficiency(cw, i) {
                None => "locked".to_string(),
                Some(eff) => format!("efficiency={eff:.2}"),
            };
            lines.push(format!(
                "gated {} on {} ({}-{}): {status}",
                def.name.replace(' ', "_"),
                gate.idea.replace(' ', "_"),
                gate.unlock_at,
                gate.full_at
            ));
        }
        Ok(lines)
    }

    fn query_population(&mut self) -> Result<Vec<String>, String> {
        let population = self.world().resource::<Population>();
        if population.individuals.is_empty() {
            Ok(vec!["(none)".to_string()])
        } else {
            Ok(population
                .individuals
                .iter()
                .enumerate()
                .map(|(i, ind)| {
                    let work = if ind.work_jobs.is_empty() {
                        "none".to_string()
                    } else {
                        ind.work_jobs
                            .iter()
                            .map(|(id, eff)| format!("{id}@{eff:.1}"))
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    format!(
                        "{i} home={} work={work} fed={} morale={:.3}",
                        ind.home()
                            .map(|h| h.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        ind.fed_fraction,
                        ind.morale()
                    )
                })
                .collect())
        }
    }

    fn query_farms(&mut self) -> Result<Vec<String>, String> {
        let farms = self.world().resource::<FarmsResource>();
        Ok(farms
            .farms
            .iter()
            .enumerate()
            .map(|(i, f)| {
                format!(
                    "{i} pos=({:.1},{:.1}) area={:.1} produces={} potatoes={} \
                     inedible={} boost={} invited={}",
                    f.seed.x,
                    f.seed.y,
                    f.area,
                    f.produced_resource().label(),
                    f.potato_stockpile,
                    f.inedible_stockpile,
                    f.boost,
                    f.invited
                )
            })
            .collect())
    }

    fn query_farm(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        let idx = parse_usize(args.first().copied().unwrap_or(""))?;
        self.world()
            .run_system_once_with(query_farm_system, idx)
            .map_err(|e| e.to_string())?
    }

    fn query_roads(&mut self) -> Result<Vec<String>, String> {
        let mut farms = self.world().resource_mut::<FarmsResource>();
        farms.ensure_roads();
        let Some(roads) = farms.roads.as_ref() else {
            return Ok(vec!["(no roads)".to_string()]);
        };
        // Summary plus a per-farm cheapest-delivery cost, so trip- and
        // paving-driven road development is observable across `advance`s.
        let developed = roads.edges.iter().filter(|e| e.trips >= 10).count();
        let paved = roads.edges.iter().filter(|e| e.paved >= 1.0).count();
        let paved_sum: f32 = roads.edges.iter().map(|e| e.paved).sum();
        let mut lines = vec![format!(
            "nodes={} edges={} city_node={} developed_edges={} paved_edges={} paved_sum={paved_sum:.3}",
            roads.nodes.len(),
            roads.edges.len(),
            roads.city_node,
            developed,
            paved,
        )];
        for (i, f) in farms.farms.iter().enumerate() {
            if let Some((weight, corner)) = roads.farm_delivery(&f.polygon) {
                let potatoes = (weight * 8.0 / 50.0f32.max(1.0)).round() as u32;
                lines.push(format!(
                    "{i} delivery_weight={weight:.1} potatoes={potatoes} corner_node={corner} \
                     route_edges={}",
                    roads.path_edges(corner).len(),
                ));
            }
        }
        Ok(lines)
    }

    fn query_outdoorness(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 3 {
            return Err("usage: query outdoorness <x> <y> <z>".to_string());
        }
        let cube = parse_ivec3(&args[0..3])?;
        let cw = self.world().resource::<ConstructedCity>();
        let map = compute_outdoorsness(&cw.contents, &cw.eorfs);
        let level = map.get(&cube).copied().unwrap_or(0.0);
        Ok(vec![format!("outdoorness={level:.3}")])
    }

    fn query_month(&mut self) -> Result<Vec<String>, String> {
        Ok(vec![format!(
            "month={}",
            self.world().resource::<GameClock>().month()
        )])
    }

    fn query_traveler(&mut self) -> Result<Vec<String>, String> {
        // Idea names come from `ConstructedCity`, so read the offer out
        // before borrowing the world a second time.
        let state = self.world().resource::<TravelerState>();
        let Some(offer) = state.current_offer.as_ref() else {
            return Ok(vec!["(none)".to_string()]);
        };
        let origin = offer.path.first().copied().unwrap_or(Vec2::ZERO);
        let invited = state.invited;
        let path_len = offer.path.len();
        let reward = offer.reward.clone();
        let reward = match &reward {
            crate::traveler::ResolvedReward::Tool(kind) => kind.label().to_string(),
            crate::traveler::ResolvedReward::Resource(res, qty) => {
                format!("{qty}_{}", res.label())
            }
            crate::traveler::ResolvedReward::Book { idea, segments, .. } => {
                let cw = self.world().resource::<ConstructedCity>();
                format!(
                    "book_{}_{}",
                    cw.ideas[*idea].name.replace(' ', "_"),
                    segments.count_ones()
                )
            }
        };
        Ok(vec![format!(
            "origin=({:.1},{:.1}) origin_dist={:.1} invited={invited} \
             path_len={path_len} reward={reward}",
            origin.x,
            origin.y,
            origin.length(),
        )])
    }

    fn query_inventory(&mut self) -> Result<Vec<String>, String> {
        let cw = self.world().resource::<ConstructedCity>();
        let mut lines = Vec::new();
        for &res in UniformResource::ALL {
            let total = crate::storage::total_uniform(cw, res);
            if total > 0 {
                lines.push(format!("{}: {total}", res.label()));
            }
        }
        // Unique resources are stored separately (racks and bookcases,
        // not bins), but they're inventory just the same.
        let tools = crate::storage::total_tools_of(cw, ToolKind::CarpentersTools);
        if tools > 0 {
            lines.push(format!("Carpenter's tools: {tools}"));
        }
        let books = crate::storage::total_book_count(cw);
        if books > 0 {
            lines.push(format!("Books: {books}"));
        }
        if lines.is_empty() {
            lines.push("(empty)".to_string());
        }
        Ok(lines)
    }

    fn query_proposals(&mut self) -> Result<Vec<String>, String> {
        let world = self.world();
        let cw = world.resource::<ConstructedCity>();
        let material_list = world.resource::<MaterialList>();
        let pending = world.resource::<ProposedCity>();
        let need =
            crate::construction::remaining_construction_need(pending, &cw.eorfs, material_list);
        Ok(vec![format!(
            "pending_changes={} remaining_need={:?}",
            pending.num_changes(),
            need
        )])
    }

    fn query_changed(&mut self) -> Result<Vec<String>, String> {
        let report = *self.world().resource::<ChangeReport>();
        Ok(vec![format_change_report(&report)])
    }

    fn query_path(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 6 {
            return Err("usage: query path <x1> <y1> <z1> <x2> <y2> <z2>".to_string());
        }
        let from = parse_ivec3(&args[0..3])?;
        let to = parse_ivec3(&args[3..6])?;
        let Some(nav) = self.world().get_resource::<NavigationGrid>() else {
            return Ok(vec![
                "no navigation grid yet (call 'tick' first)".to_string()
            ]);
        };
        match nav.find_path(from, to) {
            None => Ok(vec!["unreachable".to_string()]),
            Some(path) => Ok(vec![format!(
                "path: {}",
                path.iter()
                    .map(|c| format!("({},{},{})", c.x, c.y, c.z))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )]),
        }
    }

    fn query_connected(&mut self, args: &[&str]) -> Result<Vec<String>, String> {
        if args.len() < 6 {
            return Err("usage: query connected <x1> <y1> <z1> <x2> <y2> <z2>".to_string());
        }
        let from = parse_ivec3(&args[0..3])?;
        let to = parse_ivec3(&args[3..6])?;
        let Some(nav) = self.world().get_resource::<NavigationGrid>() else {
            return Ok(vec![
                "no navigation grid yet (call 'tick' first)".to_string()
            ]);
        };
        Ok(vec![format!("connected={}", nav.is_connected(from, to))])
    }
}

fn format_change_report(r: &ChangeReport) -> String {
    format!(
        "constructed_city={} proposed_city={} population={} farms={} nav_grid={} ideas={}",
        r.constructed_city, r.proposed_city, r.population, r.farms, r.nav_grid, r.ideas
    )
}

// ---- One-off systems, run via `World::run_system_once[_with]` ----

/// Outcome of a `place`/`remove`: how many locations changed, and whether the
/// target was silently refused because it sits in the road-forbidden zone (the
/// starting crossroads). `place_at` skips such locations without erroring, so
/// the harness reports the reason rather than leaving `changed=0` unexplained.
struct PlaceOutcome {
    changed: usize,
    blocked_by_road: bool,
}

fn place_system(
    In((loc, item, dir)): In<(SlotCoord, Option<EorfId>, i32)>,
    cw: Res<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
) -> PlaceOutcome {
    let changed = pending
        .place_at(&cw, loc, item, dir, BuildMaterialId::default())
        .len();
    let blocked_by_road = changed == 0
        && item.is_some()
        && cw.road_forbidden_zone
        && crate::road::is_in_road_forbidden_zone(loc);
    PlaceOutcome {
        changed,
        blocked_by_road,
    }
}

/// Render a `place`/`remove` outcome for the REPL, appending a diagnostic note
/// when the location was refused by the road-forbidden zone.
fn place_outcome_lines(outcome: PlaceOutcome) -> Vec<String> {
    let mut lines = vec![format!("changed={}", outcome.changed)];
    if outcome.blocked_by_road {
        lines.push(
            "note: refused -- location is in the road-forbidden zone (the starting \
             crossroads); build in the z<0 semiplane or at a higher y"
                .to_string(),
        );
    }
    lines
}

fn build_box_system(
    In((a, b)): In<(IVec3, IVec3)>,
    cw: Res<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
) -> usize {
    let built = {
        let mut builder = Builder::new(&cw.eorfs);
        builder.build_box(a, b);
        builder.get()
    };
    let mut n = 0;
    for (loc, cell) in built.iter() {
        n += pending
            .place_at(
                &cw,
                loc,
                Some(cell.id),
                cell.facing as u8 as i32,
                cell.build_material,
            )
            .len();
    }
    n
}

fn construct_system(
    mut cw: ResMut<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
    material_list: Res<MaterialList>,
) -> usize {
    let n = pending.num_changes();
    construction::construct(&mut cw, &mut pending, &material_list);
    n
}

fn undo_system(cw: Res<ConstructedCity>, mut pending: ResMut<ProposedCity>) -> usize {
    pending.undo(&cw).len()
}

fn redo_system(cw: Res<ConstructedCity>, mut pending: ResMut<ProposedCity>) -> usize {
    pending.redo(&cw).len()
}

fn load_system(
    In(new_contents): In<crate::sparse3d::Sparse3D<Cell>>,
    mut cw: ResMut<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
) {
    construction::load_from_offline(&mut cw, &mut pending, new_contents);
}

#[allow(clippy::too_many_arguments)]
fn query_farm_system(
    In(idx): In<usize>,
    mut farms: ResMut<FarmsResource>,
    cw: Res<ConstructedCity>,
    pending: Res<ProposedCity>,
    population: Res<Population>,
    traveler_state: Res<TravelerState>,
    material_list: Res<MaterialList>,
    sandbox: Res<SandboxFlag>,
) -> Result<Vec<String>, String> {
    if idx >= farms.farms.len() {
        return Err(format!("no such farm: {idx}"));
    }
    let id = FarmId::new(idx);
    farms.ensure_adjacency();
    let event = farms.farm_event(id);
    let inputs = crate::city_effect::MonthInputs {
        constructed: &cw,
        pending: &pending,
        population: &population,
        traveler_state: &traveler_state,
        material_list: &material_list,
        sandbox_enabled: sandbox.0,
    };
    let mut lines = vec![format!("farm {idx}")];
    lines.extend(farm_breakdown(&mut farms, id, event, None, inputs));
    Ok(lines)
}

/// Runs one month of game time: market, production, traveler resolution,
/// feeding, and construction progress. Mirrors the "Advance Month" action in
/// `ui.rs`'s `shared_ui_system`, minus anything visual.
fn advance_month_system(
    mut clock: ResMut<GameClock>,
    mut farms: ResMut<FarmsResource>,
    mut constructed: ResMut<ConstructedCity>,
    mut pending: ResMut<ProposedCity>,
    mut population: ResMut<Population>,
    mut traveler_state: ResMut<TravelerState>,
    mut idea_state: ResMut<crate::idea::IdeaState>,
    material_list: Res<MaterialList>,
    mut headless_rng: ResMut<HeadlessRng>,
    sandbox: Res<SandboxFlag>,
) -> Vec<String> {
    // The harness runs the same `SimulationPlugin` systems the game does, but
    // only on `tick` -- mutating commands deliberately don't advance the
    // schedule (see the module docs). So an `advance` issued straight after an
    // edit would otherwise compute the month's worker effects against stale
    // staffing; refresh it inline first.
    crate::work::assign_work(&mut population.individuals, &constructed);

    let outcome = crate::month::advance_month(
        &mut clock,
        &mut farms,
        &mut constructed,
        &mut pending,
        &mut population,
        &mut traveler_state,
        &mut idea_state,
        &material_list,
        sandbox.0,
        &mut headless_rng.0,
    );

    // Report the outcome, in the same order the fields are produced.
    let mut lines = Vec::new();
    match outcome.traveler_accepted {
        Some(true) => lines.push("traveler: accepted".to_string()),
        Some(false) => lines.push("traveler: could not afford, declined".to_string()),
        None => {}
    }
    if !outcome.market_gains.is_empty() {
        let summary = outcome
            .market_gains
            .iter()
            .map(|(r, q)| format!("{} {}", q, r.label()))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("market gains: {summary}"));
    }
    if outcome.construction_changes.is_some() {
        lines.push("construction: completed".to_string());
    }

    // Likewise, so that a `query` issued before the next `tick` reports the
    // post-month assignment rather than the pre-month one.
    assign_places(
        crate::place::AssignmentFlavor::Sleep,
        &mut population.individuals,
        &constructed,
    );
    crate::work::assign_work(&mut population.individuals, &constructed);
    lines.push(format!("month={}", clock.month()));
    lines
}

fn parse_ivec3(args: &[&str]) -> Result<IVec3, String> {
    Ok(IVec3::new(
        parse_i32(args[0])?,
        parse_i32(args[1])?,
        parse_i32(args[2])?,
    ))
}

fn parse_i32(s: &str) -> Result<i32, String> {
    s.parse().map_err(|_| format!("not an integer: {s}"))
}

fn parse_usize(s: &str) -> Result<usize, String> {
    s.parse().map_err(|_| format!("not an integer: {s}"))
}

fn parse_uniform_resource(s: &str) -> Result<UniformResource, String> {
    UniformResource::ALL
        .iter()
        .copied()
        .find(|r| format!("{r:?}").eq_ignore_ascii_case(s))
        .ok_or_else(|| format!("unknown resource: {s}"))
}

fn parse_slot(s: &str) -> Result<Slot, String> {
    match s.to_ascii_lowercase().as_str() {
        "room" => Ok(Slot::Room),
        "floor" => Ok(Slot::Floor),
        "xwall" => Ok(Slot::XLoWall),
        "zwall" => Ok(Slot::ZLoWall),
        _ => Err(format!(
            "unknown slot: {s} (expected room|floor|xwall|zwall)"
        )),
    }
}

fn parse_facing(s: &str) -> Result<i32, String> {
    match s.to_ascii_lowercase().as_str() {
        "negx" | "0" => Ok(Facing::NegX as i32),
        "negz" | "1" => Ok(Facing::NegZ as i32),
        "posx" | "2" => Ok(Facing::PosX as i32),
        "posz" | "3" => Ok(Facing::PosZ as i32),
        _ => Err(format!(
            "unknown facing: {s} (expected negx|negz|posx|posz or 0-3)"
        )),
    }
}

/// Runs the headless REPL against stdin/stdout until EOF, `quit`, or `exit`.
pub fn run() {
    let seed: u64 = std::env::args()
        .skip_while(|a| a != "--seed")
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(rand::random);

    let mut session = HeadlessSession::new(seed);
    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out, "OK ready seed={seed}").ok();
    writeln!(out).ok();
    out.flush().ok();

    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "quit" || trimmed == "exit" {
            break;
        }

        match session.dispatch(trimmed) {
            Ok(lines) => {
                writeln!(out, "OK").ok();
                for l in lines {
                    writeln!(out, "{l}").ok();
                }
            }
            Err(e) => {
                writeln!(out, "ERR {e}").ok();
            }
        }
        writeln!(out).ok();
        out.flush().ok();
    }
}

/// End-to-end tests of construction operations, driven entirely through
/// [`HeadlessSession::dispatch`] the same way a scripted session would --
/// exercising the resource-payment rework (propose, pay down incrementally
/// via the market, force-commit, undo/redo) rather than calling internal
/// systems directly.
#[cfg(test)]
mod tests {
    use super::*;

    /// Dispatches `cmd`, panicking with the session's error on failure -- test
    /// bodies read as the command script they are.
    fn dispatch_ok(session: &mut HeadlessSession, cmd: &str) -> Vec<String> {
        session
            .dispatch(cmd)
            .unwrap_or_else(|e| panic!("{cmd}: {e}"))
    }

    /// The initial farm layout (positions, resources, stockpiles -- see
    /// `map::build_farms_resource`) is drawn from the session's seeded RNG, so
    /// two sessions opened with the same `--seed` must produce an identical
    /// `query farms` listing, and different seeds should (overwhelmingly
    /// likely) differ.
    #[test]
    fn farm_generation_is_seeded() {
        let mut a = HeadlessSession::new(42);
        let mut b = HeadlessSession::new(42);
        assert_eq!(
            dispatch_ok(&mut a, "query farms"),
            dispatch_ok(&mut b, "query farms"),
            "same seed must produce the same farm layout"
        );

        let mut c = HeadlessSession::new(43);
        assert_ne!(
            dispatch_ok(&mut a, "query farms"),
            dispatch_ok(&mut c, "query farms"),
            "different seeds should not produce the same farm layout"
        );
    }

    /// Indices of farms whose `produces=` field in `query farms` matches
    /// `resource` (e.g. "Straw").
    fn farms_producing(session: &mut HeadlessSession, resource: &str) -> Vec<usize> {
        dispatch_ok(session, "query farms")
            .iter()
            .filter_map(|line| {
                let idx: usize = line.split_whitespace().next()?.parse().ok()?;
                let produces = line
                    .split_whitespace()
                    .find_map(|tok| tok.strip_prefix("produces="))?;
                (produces == resource).then_some(idx)
            })
            .collect()
    }

    fn invite_all(session: &mut HeadlessSession, indices: &[usize]) {
        for &idx in indices {
            dispatch_ok(session, &format!("invite {idx}"));
        }
    }

    /// `pallet` (a furniture piece with a fixed, material-independent cost of
    /// 2 straw + 2 canvas, both directly farmable) is used throughout as the
    /// cheapest structure whose cost can actually be paid off by the market --
    /// walls/pillars/floors go through `BuildMaterialId::default()` (Ashlar,
    /// priced in Block), and nothing in the game currently produces Block.
    const PALLET_CELL: &str = "100 0 100 room";

    #[test]
    fn sandbox_build_skips_resource_payment() {
        let mut session = HeadlessSession::new(1);
        // Sandbox is on by default: placing commits immediately, for free.
        dispatch_ok(&mut session, &format!("place {PALLET_CELL} pallet"));
        dispatch_ok(&mut session, "set_inventory potato 0");
        dispatch_ok(&mut session, "set_inventory canvas 0");
        dispatch_ok(&mut session, "set_inventory plank 0");

        let cell = dispatch_ok(&mut session, &format!("query cell {PALLET_CELL}"));
        assert!(
            cell[0].starts_with("real: pallet"),
            "expected an immediately-built pallet: {cell:?}"
        );

        let proposals = dispatch_ok(&mut session, "query proposals");
        assert_eq!(proposals[0], "pending_changes=0 remaining_need=[]");

        // No storage exists and nothing was paid for, so inventory stays empty.
        assert_eq!(
            dispatch_ok(&mut session, "query inventory"),
            vec!["(empty)".to_string()]
        );
    }

    #[test]
    fn non_sandbox_place_proposes_and_tracks_remaining_need() {
        let mut session = HeadlessSession::new(2);
        dispatch_ok(&mut session, "sandbox off");
        dispatch_ok(&mut session, &format!("place {PALLET_CELL} pallet"));

        let cell = dispatch_ok(&mut session, &format!("query cell {PALLET_CELL}"));
        assert_eq!(cell[0], "real: empty");
        assert!(cell[1].starts_with("proposed: add pallet"), "{cell:?}");

        let proposals = dispatch_ok(&mut session, "query proposals");
        assert_eq!(
            proposals[0], "pending_changes=1 remaining_need=[(Straw, 2), (Canvas, 2)]",
            "a pallet costs exactly 2 straw + 2 canvas: {proposals:?}"
        );
    }

    #[test]
    fn undo_redo_round_trips_a_pending_proposal() {
        let mut session = HeadlessSession::new(3);
        dispatch_ok(&mut session, "sandbox off");
        dispatch_ok(&mut session, &format!("place {PALLET_CELL} pallet"));
        assert!(dispatch_ok(&mut session, "query proposals")[0].starts_with("pending_changes=1"));

        dispatch_ok(&mut session, "undo");
        assert_eq!(
            dispatch_ok(&mut session, "query proposals")[0],
            "pending_changes=0 remaining_need=[]"
        );

        dispatch_ok(&mut session, "redo");
        assert!(dispatch_ok(&mut session, "query proposals")[0].starts_with("pending_changes=1"));
    }

    #[test]
    fn construct_command_force_commits_regardless_of_payment() {
        let mut session = HeadlessSession::new(4);
        dispatch_ok(&mut session, "sandbox off");
        dispatch_ok(&mut session, &format!("place {PALLET_CELL} pallet"));
        // Nothing has been paid off yet, but `construct` is a manual override
        // (mirrors what sandbox mode does automatically).
        dispatch_ok(&mut session, "construct");

        let cell = dispatch_ok(&mut session, &format!("query cell {PALLET_CELL}"));
        assert!(cell[0].starts_with("real: pallet"), "{cell:?}");
        assert_eq!(
            dispatch_ok(&mut session, "query proposals")[0],
            "pending_changes=0 remaining_need=[]"
        );
    }

    #[test]
    fn advance_withholds_construction_until_resources_are_delivered() {
        let mut session = HeadlessSession::new(5);
        dispatch_ok(&mut session, "sandbox off");
        dispatch_ok(&mut session, &format!("place {PALLET_CELL} pallet"));
        dispatch_ok(&mut session, "set_inventory canvas 0");
        dispatch_ok(&mut session, "set_inventory straw 0");
        let before = dispatch_ok(&mut session, "query proposals");
        assert_eq!(
            before[0],
            "pending_changes=1 remaining_need=[(Straw, 2), (Canvas, 2)]"
        );

        // No farms invited: nothing arrives at the market, so nothing gets
        // applied toward the cost.
        dispatch_ok(&mut session, "advance");
        let after = dispatch_ok(&mut session, "query proposals");
        assert_eq!(
            after, before,
            "remaining need shouldn't move without any market gains"
        );

        // Invite every farm producing either needed resource and retry,
        // re-inviting each month since invitations reset once the market runs.
        let straw = farms_producing(&mut session, "Straw");
        let canvas = farms_producing(&mut session, "Canvas");
        assert!(
            !straw.is_empty() && !canvas.is_empty(),
            "expected at least one farm producing each of straw and canvas"
        );

        let mut completed = false;
        for _ in 0..8 {
            invite_all(&mut session, &straw);
            invite_all(&mut session, &canvas);
            let lines = dispatch_ok(&mut session, "advance");
            if lines.iter().any(|l| l == "construction: completed") {
                completed = true;
                break;
            }
        }
        assert!(
            completed,
            "construction should complete once enough straw/canvas reached the market"
        );

        let cell = dispatch_ok(&mut session, &format!("query cell {PALLET_CELL}"));
        assert!(cell[0].starts_with("real: pallet"), "{cell:?}");
        assert_eq!(
            dispatch_ok(&mut session, "query proposals")[0],
            "pending_changes=0 remaining_need=[]"
        );

        // The starting camp's wagon provides some storage, so leftover
        // resources beyond what construction consumed were stockpiled there
        // rather than lost. Exact amounts depend on which farms happened to
        // be invited across the retry loop above, so just check something
        // landed in storage instead of everything being lost.
        assert_ne!(
            dispatch_ok(&mut session, "query inventory"),
            vec!["(empty)".to_string()]
        );
    }
}
