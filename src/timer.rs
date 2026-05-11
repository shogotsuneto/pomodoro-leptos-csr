pub const WORK_SECS: u32 = 25 * 60;
pub const BREAK_SECS: u32 = 5 * 60;

#[derive(Clone, Copy, PartialEq)]
pub enum Phase {
    Work,
    Break,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Work => "Work",
            Phase::Break => "Break",
        }
    }

    pub fn duration_secs(self) -> u32 {
        match self {
            Phase::Work => WORK_SECS,
            Phase::Break => BREAK_SECS,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Phase::Work => Phase::Break,
            Phase::Break => Phase::Work,
        }
    }
}
