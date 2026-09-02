use ratatui::style::Color;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Theme {
    #[default]
    Rainbow,
    Midnight,
    Mono,
}

impl Theme {
    pub const fn next(self) -> Self {
        match self {
            Self::Rainbow => Self::Midnight,
            Self::Midnight => Self::Mono,
            Self::Mono => Self::Rainbow,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Rainbow => "Rainbow",
            Self::Midnight => "Midnight",
            Self::Mono => "Mono",
        }
    }

    pub const fn palette(self) -> Palette {
        match self {
            Self::Rainbow => Palette {
                background: Color::Rgb(8, 9, 20),
                surface: Color::Rgb(25, 20, 43),
                border: Color::Rgb(78, 65, 110),
                text: Color::Rgb(245, 243, 255),
                muted: Color::Rgb(169, 159, 193),
                healthy: Color::Rgb(80, 235, 190),
                time: Color::Rgb(64, 214, 255),
                warning: Color::Rgb(255, 202, 87),
                critical: Color::Rgb(255, 91, 146),
                openai: Color::Rgb(80, 235, 190),
                anthropic: Color::Rgb(255, 143, 96),
                google: Color::Rgb(91, 157, 255),
                cursor: Color::Rgb(181, 117, 255),
                github: Color::Rgb(255, 99, 190),
                spark_short: Color::Rgb(181, 117, 255),
                spark_long: Color::Rgb(255, 99, 190),
            },
            Self::Midnight => Palette {
                background: Color::Rgb(8, 12, 18),
                surface: Color::Rgb(18, 25, 35),
                border: Color::Rgb(54, 68, 84),
                text: Color::Rgb(226, 232, 240),
                muted: Color::Rgb(139, 153, 170),
                healthy: Color::Rgb(67, 190, 132),
                time: Color::Rgb(96, 165, 250),
                warning: Color::Rgb(232, 184, 80),
                critical: Color::Rgb(235, 98, 98),
                openai: Color::Rgb(16, 163, 127),
                anthropic: Color::Rgb(217, 119, 87),
                google: Color::Rgb(66, 133, 244),
                cursor: Color::Rgb(167, 139, 250),
                github: Color::Rgb(163, 113, 247),
                spark_short: Color::Rgb(42, 183, 184),
                spark_long: Color::Rgb(64, 145, 214),
            },
            Self::Mono => Palette {
                background: Color::Rgb(10, 10, 10),
                surface: Color::Rgb(32, 32, 32),
                border: Color::Rgb(76, 76, 76),
                text: Color::Rgb(238, 238, 238),
                muted: Color::Rgb(158, 158, 158),
                healthy: Color::Rgb(218, 218, 218),
                time: Color::Rgb(158, 158, 158),
                warning: Color::Rgb(218, 218, 218),
                critical: Color::Rgb(255, 255, 255),
                openai: Color::Rgb(238, 238, 238),
                anthropic: Color::Rgb(238, 238, 238),
                google: Color::Rgb(238, 238, 238),
                cursor: Color::Rgb(238, 238, 238),
                github: Color::Rgb(238, 238, 238),
                spark_short: Color::Rgb(204, 204, 204),
                spark_long: Color::Rgb(172, 172, 172),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub healthy: Color,
    pub time: Color,
    pub warning: Color,
    pub critical: Color,
    pub openai: Color,
    pub anthropic: Color,
    pub google: Color,
    pub cursor: Color,
    pub github: Color,
    pub spark_short: Color,
    pub spark_long: Color,
}

pub const fn provider_accent(provider_id: &str, palette: Palette) -> Color {
    match provider_id.as_bytes() {
        b"openai" => palette.openai,
        b"anthropic" => palette.anthropic,
        b"google" => palette.google,
        b"cursor" => palette.cursor,
        b"github" => palette.github,
        _ => palette.border,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rainbow_is_the_default_and_theme_cycle_wraps() {
        assert_eq!(Theme::default(), Theme::Rainbow);
        assert_eq!(Theme::Rainbow.next(), Theme::Midnight);
        assert_eq!(Theme::Midnight.next(), Theme::Mono);
        assert_eq!(Theme::Mono.next(), Theme::Rainbow);
    }

    #[test]
    fn rainbow_uses_distinct_provider_and_window_accents() {
        let palette = Theme::Rainbow.palette();
        assert_eq!(provider_accent("openai", palette), palette.openai);
        assert_eq!(provider_accent("anthropic", palette), palette.anthropic);
        assert_eq!(provider_accent("unknown", palette), palette.border);
        assert_ne!(palette.openai, palette.spark_short);
        assert_ne!(palette.spark_short, palette.spark_long);
    }

    #[test]
    fn themes_have_distinct_surfaces() {
        let rainbow = Theme::Rainbow.palette();
        let midnight = Theme::Midnight.palette();
        let mono = Theme::Mono.palette();
        assert_ne!(rainbow.surface, midnight.surface);
        assert_ne!(midnight.surface, mono.surface);
        assert_ne!(rainbow.surface, mono.surface);
    }
}
