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

    pub fn next(self) -> Self {
        match self {
            Phase::Work => Phase::Break,
            Phase::Break => Phase::Work,
        }
    }
}
