use ratatui::style::Color;

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub healthy: Color,
    pub warning: Color,
    pub critical: Color,
}

pub fn palette() -> Palette {
    Palette {
        background: Color::Rgb(8, 12, 18),
        surface: Color::Rgb(18, 25, 35),
        border: Color::Rgb(54, 68, 84),
        text: Color::Rgb(226, 232, 240),
        muted: Color::Rgb(139, 153, 170),
        healthy: Color::Rgb(67, 190, 132),
        warning: Color::Rgb(232, 184, 80),
        critical: Color::Rgb(235, 98, 98),
    }
}

pub fn provider_accent(provider_id: &str, palette: Palette) -> Color {
    match provider_id {
        "openai" => Color::Rgb(16, 163, 127),
        "anthropic" => Color::Rgb(217, 119, 87),
        "google" => Color::Rgb(66, 133, 244),
        "cursor" => Color::Rgb(167, 139, 250),
        "github" => Color::Rgb(163, 113, 247),
        _ => palette.border,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_providers_have_stable_accents() {
        let palette = palette();
        assert_eq!(provider_accent("openai", palette), Color::Rgb(16, 163, 127));
        assert_eq!(
            provider_accent("anthropic", palette),
            Color::Rgb(217, 119, 87)
        );
        assert_eq!(provider_accent("google", palette), Color::Rgb(66, 133, 244));
        assert_eq!(
            provider_accent("cursor", palette),
            Color::Rgb(167, 139, 250)
        );
        assert_eq!(
            provider_accent("github", palette),
            Color::Rgb(163, 113, 247)
        );
        assert_eq!(provider_accent("unknown", palette), palette.border);
    }
}
