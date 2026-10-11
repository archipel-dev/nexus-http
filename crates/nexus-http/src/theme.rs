//! Colors and fonts derived from the host's egui visuals, so the plugin follows
//! the workbench theme (Islands Dark / Light) without depending on it.

use egui::{Color32, FontFamily, FontId, Stroke, Ui};
use nexus_http_core::model::Method;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// Background between islands (window frame).
    pub frame: Color32,
    /// Background of the main surface (island).
    pub surface: Color32,
    /// Slightly raised background (sidebar, headers).
    pub raised: Color32,
    /// Input field background.
    pub field: Color32,
    pub border: Color32,
    pub text: Color32,
    pub strong: Color32,
    pub weak: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub selection: Color32,
    pub hover: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub info: Color32,
    pub purple: Color32,
    /// JSON highlighting.
    pub syntax_key: Color32,
    pub syntax_string: Color32,
    pub syntax_number: Color32,
    pub syntax_keyword: Color32,
    pub syntax_punct: Color32,
}

impl Palette {
    pub fn of(ui: &Ui) -> Self {
        Self::from_visuals(ui.visuals())
    }

    pub fn from_visuals(v: &egui::Visuals) -> Self {
        let dark = v.dark_mode;
        let pick = |d: u32, l: u32| hex(if dark { d } else { l });
        Self {
            dark,
            frame: v.panel_fill,
            surface: v.code_bg_color,
            raised: v.window_fill,
            field: v.extreme_bg_color,
            border: v.widgets.noninteractive.bg_stroke.color,
            text: v.text_color(),
            strong: v.strong_text_color(),
            weak: v.weak_text_color(),
            faint: v.weak_text_color().gamma_multiply(0.7),
            accent: pick(0x548AF7, 0x3574F0),
            selection: v.selection.bg_fill,
            hover: v.widgets.hovered.weak_bg_fill,
            success: pick(0x6DB083, 0x208A3C),
            warning: pick(0xD59637, 0xB46B00),
            error: pick(0xF57E84, 0xD03D4C),
            info: pick(0x71A1FE, 0x3574F0),
            purple: pick(0xB589EC, 0x834DF0),
            syntax_key: pick(0xC77DBB, 0x871094),
            syntax_string: pick(0x6AAB73, 0x067D17),
            syntax_number: pick(0x2AACB8, 0x1750EB),
            syntax_keyword: pick(0xCF8E6D, 0x0033B3),
            syntax_punct: pick(0xA9B7C6, 0x6F737A),
        }
    }

    pub fn method(&self, method: &Method) -> Color32 {
        match method {
            Method::Get => self.success,
            Method::Post => self.warning,
            Method::Put => self.info,
            Method::Patch => self.purple,
            Method::Delete => self.error,
            Method::Head | Method::Options | Method::Custom(_) => self.weak,
        }
    }

    pub fn status(&self, status: u16) -> Color32 {
        match status {
            100..=199 => self.info,
            200..=299 => self.success,
            300..=399 => self.purple,
            400..=499 => self.warning,
            _ => self.error,
        }
    }

    pub fn hairline(&self) -> Stroke {
        Stroke::new(1.0_f32, self.border)
    }
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Short method label used in the sidebar (`DEL`, `OPT`).
pub fn method_short(method: &Method) -> String {
    match method {
        Method::Delete => "DEL".into(),
        Method::Options => "OPT".into(),
        Method::Patch => "PTCH".into(),
        Method::Custom(m) => m.chars().take(4).collect(),
        other => other.as_str().to_owned(),
    }
}

/// Monospace font at `size`.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Semibold UI font when the host installed one (the Archipel kit does), else regular.
pub fn semibold(ui: &Ui, size: f32) -> FontId {
    let family = FontFamily::Name("semibold".into());
    let installed = ui.fonts(|f| f.families().contains(&family));
    FontId::new(
        size,
        if installed {
            family
        } else {
            FontFamily::Proportional
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_differ_between_light_and_dark() {
        let dark = Palette::from_visuals(&egui::Visuals::dark());
        let light = Palette::from_visuals(&egui::Visuals::light());
        assert!(dark.dark && !light.dark);
        assert_ne!(dark.success, light.success);
        assert_eq!(dark.method(&Method::Get), dark.success);
        assert_eq!(dark.status(404), dark.warning);
        assert_eq!(method_short(&Method::Delete), "DEL");
    }
}
