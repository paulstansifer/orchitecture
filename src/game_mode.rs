use bevy::prelude::*;

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GameMode {
    #[default]
    Build,
    Walk,
    Surroundings,
}

/// When enabled, construction edits commit immediately instead of becoming
/// proposals, and (eventually) edits are free. Loading structures is only
/// available in sandbox mode. Enabled on startup.
#[derive(Resource)]
pub struct SandboxMode {
    pub enabled: bool,
}

impl Default for SandboxMode {
    fn default() -> Self {
        SandboxMode { enabled: true }
    }
}
