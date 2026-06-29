//! Color palettes and gradient helpers for the visualizer.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    Spectrum,
    Rainbow,
    Fire,
    Lava,
    Sunset,
    Gold,
    Aurora,
    Forest,
    Matrix,
    Ocean,
    Ice,
    Neon,
    Synthwave,
    Vaporwave,
    Candy,
    Plasma,
    Viridis,
    Magma,
    Cyberpunk,
    Ember,
    Mint,
    Sakura,
    Dracula,
    Nord,
    Gruvbox,
    Solar,
    Mono,
}

impl Palette {
    /// All palettes, in cycle order.
    pub const ALL: [Palette; 27] = [
        Palette::Spectrum,
        Palette::Rainbow,
        Palette::Fire,
        Palette::Lava,
        Palette::Sunset,
        Palette::Gold,
        Palette::Aurora,
        Palette::Forest,
        Palette::Matrix,
        Palette::Ocean,
        Palette::Ice,
        Palette::Neon,
        Palette::Synthwave,
        Palette::Vaporwave,
        Palette::Candy,
        Palette::Plasma,
        Palette::Viridis,
        Palette::Magma,
        Palette::Cyberpunk,
        Palette::Ember,
        Palette::Mint,
        Palette::Sakura,
        Palette::Dracula,
        Palette::Nord,
        Palette::Gruvbox,
        Palette::Solar,
        Palette::Mono,
    ];

    /// Index in `ALL` (for pickers).
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }

    /// A vivid, representative accent colour for theming the UI chrome.
    pub fn accent(self) -> Color {
        self.color(0.82)
    }

    /// A softer tint of the accent, for secondary text/labels.
    pub fn accent_dim(self) -> Color {
        if let Color::Rgb(r, g, b) = self.accent() {
            // Blend toward a neutral grey so it reads as a muted label.
            let mix = |c: u8, n: u8| ((c as u16 * 6 + n as u16 * 4) / 10) as u8;
            Color::Rgb(mix(r, 150), mix(g, 150), mix(b, 160))
        } else {
            Color::Rgb(150, 150, 160)
        }
    }

    pub fn name(self) -> &'static str {
        use Palette::*;
        match self {
            Spectrum => "spectrum",
            Rainbow => "rainbow",
            Fire => "fire",
            Lava => "lava",
            Sunset => "sunset",
            Gold => "gold",
            Aurora => "aurora",
            Forest => "forest",
            Matrix => "matrix",
            Ocean => "ocean",
            Ice => "ice",
            Neon => "neon",
            Synthwave => "synthwave",
            Vaporwave => "vaporwave",
            Candy => "candy",
            Plasma => "plasma",
            Viridis => "viridis",
            Magma => "magma",
            Cyberpunk => "cyberpunk",
            Ember => "ember",
            Mint => "mint",
            Sakura => "sakura",
            Dracula => "dracula",
            Nord => "nord",
            Gruvbox => "gruvbox",
            Solar => "solar",
            Mono => "mono",
        }
    }

    /// Color stops for this palette, ordered from low (t=0) to high (t=1).
    fn stops(self) -> &'static [[u8; 3]] {
        use Palette::*;
        match self {
            Spectrum => &[
                [20, 40, 120],
                [0, 160, 200],
                [0, 220, 140],
                [230, 220, 40],
                [240, 90, 30],
                [240, 40, 80],
            ],
            Rainbow => &[
                [230, 40, 60],
                [240, 150, 30],
                [230, 220, 40],
                [40, 200, 90],
                [40, 130, 230],
                [150, 60, 220],
            ],
            Fire => &[
                [20, 8, 4],
                [120, 20, 0],
                [220, 70, 0],
                [250, 160, 20],
                [255, 240, 160],
            ],
            Lava => &[
                [10, 4, 4],
                [90, 10, 6],
                [200, 30, 10],
                [255, 110, 20],
                [255, 220, 90],
            ],
            Sunset => &[
                [30, 20, 70],
                [120, 40, 120],
                [220, 70, 110],
                [250, 140, 70],
                [255, 215, 120],
            ],
            Gold => &[
                [40, 24, 6],
                [120, 80, 20],
                [200, 150, 40],
                [240, 200, 90],
                [255, 245, 200],
            ],
            Aurora => &[
                [10, 10, 30],
                [40, 20, 90],
                [30, 140, 120],
                [120, 230, 120],
                [220, 250, 180],
            ],
            Forest => &[
                [8, 24, 10],
                [20, 70, 30],
                [60, 140, 50],
                [150, 210, 80],
                [225, 245, 170],
            ],
            Matrix => &[
                [0, 12, 0],
                [0, 60, 12],
                [0, 140, 30],
                [40, 220, 70],
                [180, 255, 180],
            ],
            Ocean => &[
                [4, 12, 40],
                [10, 60, 120],
                [10, 130, 180],
                [80, 200, 210],
                [200, 250, 240],
            ],
            Ice => &[
                [10, 20, 50],
                [30, 80, 150],
                [80, 160, 220],
                [160, 220, 245],
                [235, 250, 255],
            ],
            Neon => &[
                [255, 20, 150],
                [200, 40, 255],
                [40, 120, 255],
                [0, 230, 220],
                [120, 255, 120],
            ],
            Synthwave => &[
                [20, 10, 50],
                [90, 20, 130],
                [220, 40, 150],
                [255, 100, 90],
                [255, 200, 80],
            ],
            Vaporwave => &[
                [40, 220, 220],
                [120, 200, 255],
                [220, 150, 255],
                [255, 140, 220],
                [255, 200, 235],
            ],
            Candy => &[
                [255, 90, 160],
                [255, 140, 90],
                [255, 230, 110],
                [140, 230, 160],
                [120, 200, 255],
            ],
            Plasma => &[
                [13, 8, 135],
                [126, 3, 168],
                [204, 71, 120],
                [248, 149, 64],
                [240, 249, 33],
            ],
            Viridis => &[
                [68, 1, 84],
                [59, 82, 139],
                [33, 145, 140],
                [94, 201, 98],
                [253, 231, 37],
            ],
            Magma => &[
                [0, 0, 4],
                [81, 18, 124],
                [183, 55, 121],
                [252, 137, 97],
                [252, 253, 191],
            ],
            Cyberpunk => &[
                [10, 6, 40],
                [70, 10, 130],
                [220, 20, 160],
                [0, 200, 230],
                [250, 240, 60],
            ],
            Ember => &[
                [18, 6, 10],
                [80, 14, 24],
                [180, 40, 40],
                [240, 110, 50],
                [255, 200, 120],
            ],
            Mint => &[
                [6, 30, 26],
                [16, 90, 80],
                [40, 170, 140],
                [120, 220, 180],
                [220, 255, 235],
            ],
            Sakura => &[
                [60, 20, 50],
                [150, 50, 100],
                [230, 110, 160],
                [255, 170, 200],
                [255, 225, 235],
            ],
            Dracula => &[
                [40, 42, 54],
                [98, 114, 164],
                [189, 147, 249],
                [255, 121, 198],
                [241, 250, 140],
            ],
            Nord => &[
                [46, 52, 64],
                [76, 86, 106],
                [94, 129, 172],
                [136, 192, 208],
                [216, 222, 233],
            ],
            Gruvbox => &[
                [40, 36, 33],
                [152, 151, 26],
                [215, 153, 33],
                [214, 93, 14],
                [251, 241, 199],
            ],
            Solar => &[
                [0, 43, 54],
                [38, 139, 210],
                [42, 161, 152],
                [181, 137, 0],
                [203, 75, 22],
            ],
            Mono => &[[30, 30, 36], [120, 120, 140], [235, 235, 245]],
        }
    }

    /// Map t in 0..=1 to an RGB color along the palette gradient.
    pub fn color(self, t: f32) -> Color {
        let stops = self.stops();
        let t = t.clamp(0.0, 1.0);
        if stops.len() == 1 {
            let c = stops[0];
            return Color::Rgb(c[0], c[1], c[2]);
        }
        let scaled = t * (stops.len() - 1) as f32;
        let idx = (scaled.floor() as usize).min(stops.len() - 2);
        let frac = scaled - idx as f32;
        let a = stops[idx];
        let b = stops[idx + 1];
        let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * frac).round() as u8;
        Color::Rgb(lerp(a[0], b[0]), lerp(a[1], b[1]), lerp(a[2], b[2]))
    }
}
