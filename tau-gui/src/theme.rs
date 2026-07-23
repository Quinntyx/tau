use gpui::{Rgba, WindowAppearance, rgb};

#[derive(Clone, Copy)]
pub struct Theme {
    pub canvas: Rgba,
    pub toolbar: Rgba,
    pub sidebar: Rgba,
    pub surface: Rgba,
    pub elevated: Rgba,
    pub text: Rgba,
    pub secondary_text: Rgba,
    pub tertiary_text: Rgba,
    pub separator: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub selection: Rgba,
    pub error_surface: Rgba,
    pub error_text: Rgba,
    pub success_surface: Rgba,
    pub success_text: Rgba,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self {
                canvas: rgb(0xf8f9fa),
                toolbar: rgb(0xffffff),
                sidebar: rgb(0xf1f3f5),
                surface: rgb(0xffffff),
                elevated: rgb(0xf1f3f5),
                text: rgb(0x1f2328),
                secondary_text: rgb(0x656d76),
                tertiary_text: rgb(0x8c959f),
                separator: rgb(0xd0d7de),
                accent: rgb(0x007aff),
                accent_hover: rgb(0x006ee6),
                selection: rgb(0xddf4ff),
                error_surface: rgb(0xffebec),
                error_text: rgb(0xd1242f),
                success_surface: rgb(0xdafbe1),
                success_text: rgb(0x1a7f37),
            },
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
        }
    }

    pub fn dark() -> Self {
        Self {
            canvas: rgb(0x171719),
            toolbar: rgb(0x202023),
            sidebar: rgb(0x222225),
            surface: rgb(0x2a2a2e),
            elevated: rgb(0x323237),
            text: rgb(0xf5f5f7),
            secondary_text: rgb(0xaeaeb2),
            tertiary_text: rgb(0x8e8e93),
            separator: rgb(0x3a3a3c),
            accent: rgb(0x0a84ff),
            accent_hover: rgb(0x409cff),
            selection: rgb(0x193a5f),
            error_surface: rgb(0x4a2426),
            error_text: rgb(0xffb4ab),
            success_surface: rgb(0x203a29),
            success_text: rgb(0x9fe3b1),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_appearances_produce_distinct_accessible_hierarchies() {
        let light = Theme::for_appearance(WindowAppearance::Light);
        let dark = Theme::for_appearance(WindowAppearance::Dark);
        assert_ne!(light.canvas, dark.canvas);
        assert_ne!(light.text, dark.text);
        assert_ne!(light.sidebar, light.surface);
        assert_ne!(dark.sidebar, dark.surface);
        assert_eq!(light.accent, rgb(0x007aff));
        assert_eq!(dark.accent, rgb(0x0a84ff));
    }
}
