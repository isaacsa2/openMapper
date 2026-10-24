//! How much the view draws: the sun's shadows, edge smoothing (MSAA) and how far it sees.
//! Lower settings for slower computers (View > Graphics quality).

use crate::i18n::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Low, Quality::Medium, Quality::High];

    /// From the settings (`OPENMAPPER_GRAPHICS` wins, for checks); Medium when not set.
    pub fn from_code(c: &str) -> Quality {
        let env = std::env::var("OPENMAPPER_GRAPHICS").unwrap_or_default();
        match if env.is_empty() { c } else { env.as_str() } {
            "low" => Quality::Low,
            "medium" | "" => Quality::Medium,
            _ => Quality::High,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => tr("Low (no shadows, rough edges, 3 km view)", "Низкое (без теней, грубые края, обзор 3 км)"),
            Quality::Medium => tr("Medium (softer shadows, 8 km view)", "Среднее (тени проще, обзор 8 км)"),
            Quality::High => tr("High", "Высокое"),
        }
    }

    /// Texels a side of the sun's shadow map (0: no shadows).
    pub fn shadow_size(self) -> u32 {
        match self {
            Quality::Low => 0,
            Quality::Medium => 2048,
            Quality::High => 4096,
        }
    }

    /// How far round the camera the sun's shadows reach at most (m, half the square): seen
    /// from far off, a wider one only costs time, its texels too coarse to show anything.
    pub fn shadow_reach(self) -> f32 {
        match self {
            Quality::Low => 0.0,
            Quality::Medium => 800.0,
            Quality::High => 1500.0,
        }
    }

    /// How far small things are drawn, in multiples of their size (see `cull::LOD_RADII`).
    pub fn detail(self) -> f32 {
        match self {
            Quality::Low => super::cull::LOD_RADII * 0.25,
            Quality::Medium => super::cull::LOD_RADII * 0.5,
            Quality::High => super::cull::LOD_RADII,
        }
    }

    /// Samples a pixel (edge smoothing).
    pub fn samples(self) -> u32 {
        match self {
            Quality::Low => 1,
            Quality::Medium | Quality::High => 4,
        }
    }

    /// How far things are drawn (m).
    pub fn view_distance(self) -> f32 {
        match self {
            Quality::Low => 3000.0,
            Quality::Medium => 8000.0,
            Quality::High => f32::INFINITY,
        }
    }
}
