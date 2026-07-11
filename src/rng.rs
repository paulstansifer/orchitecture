//! The game's single source of randomness.
//!
//! Every game-affecting random draw -- initial farm layout ([`generate_farms`],
//! monthly market/production/traveler rolls ([`crate::month::advance_month`]) --
//! pulls from this one [`GameRng`] resource, so a session is fully reproducible
//! from the seed it was created with. Headless mode ([`crate::headless`]) seeds
//! it from `--seed`; the graphical app seeds it from OS entropy.
//!
//! [`generate_farms`]: crate::surroundings::map::generate_farms

use bevy::prelude::Resource;
use rand::{rngs::StdRng, SeedableRng};

/// The game's shared, seedable RNG. Insert this before any system that draws
/// randomness runs (farm generation at startup, month advancement thereafter).
#[derive(Resource)]
pub struct GameRng(pub StdRng);

impl GameRng {
    /// A deterministic RNG derived from `seed` -- the same seed always yields
    /// the same game.
    pub fn seeded(seed: u64) -> Self {
        GameRng(StdRng::seed_from_u64(seed))
    }

    /// A fresh RNG seeded from OS entropy, for a non-reproducible session.
    pub fn from_entropy() -> Self {
        GameRng(StdRng::from_os_rng())
    }
}
