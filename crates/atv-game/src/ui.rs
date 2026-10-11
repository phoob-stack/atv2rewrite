//! The game's own 2D art: texture pages and their `.bed` sprite sheets, and
//! the bitmap fonts, all read from the player's disc and drawn with Bevy UI.
//!
//! Layout works in the original's 640x480 screen units: [`UiScale`] maps
//! 480 units to the window height, so the HUD and menus keep their size on
//! any resolution and widescreen just adds room at the sides.

use std::collections::HashMap;

use anyhow::Result;
use bevy::asset::RenderAssetUsages;
use bevy::input::gamepad::GamepadButton;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::widget::NodeImageMode;
use bevy::window::PrimaryWindow;

use atv_formats::bed::{Sprite, SpriteSheet};
use atv_formats::GameSource;

use crate::controller::PadStyle;

/// Screen height the original UI was laid out for.
pub const SCREEN_H: f32 = 480.0;
pub const SCREEN_W: f32 = 640.0;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (fit_ui_scale, refresh_menus, restyle_hints, layout_bitmap_text.in_set(TextLayout)).chain().before(bevy::ui::UiSystems::Prepare));
    }
}

/// A pad button's picture in a row of button hints, drawn as on the pad
/// last used (see [`UiAssets::button`]).
#[derive(Component, Clone, Copy)]
pub struct HintButton {
    pub button: GamepadButton,
    pub height: f32,
}

impl HintButton {
    /// The picture, `height` units tall (drawn by [`restyle_hints`]).
    pub fn new(button: GamepadButton, height: f32) -> (Node, ImageNode, Self) {
        (Node::default(), ImageNode::default(), Self { button, height })
    }
}

/// Draws new button hints, and redraws them all when a pad of another
/// make is picked up.
fn restyle_hints(ports: Option<Res<crate::controller::Ports>>, ui: Option<Res<UiAssets>>, mut hints: Query<(Ref<HintButton>, &mut Node, &mut ImageNode)>) {
    let Some(ui) = ui else { return };
    let style = ports.as_ref().map_or(PadStyle::Xbox, |p| p.last);
    let all = ports.is_some_and(|p| p.is_changed());
    for (hint, mut node, mut img) in &mut hints {
        if all || hint.is_added() {
            (*node, *img) = ui.button_node(style, hint.button, hint.height);
        }
    }
}

/// Lays out changed [`BitmapText`]. A `PostUpdate` system that changes
/// text or despawns screens goes before it, so a screen's glyphs are never
/// rebuilt under a parent despawned the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextLayout;

fn fit_ui_scale(windows: Query<&Window, With<PrimaryWindow>>, mut scale: ResMut<UiScale>) {
    if let Ok(w) = windows.single() {
        let s = (w.height() / SCREEN_H).max(0.1);
        if (scale.0 - s).abs() > 1e-4 {
            scale.0 = s;
        }
    }
}

/// A decoded texture page.
#[derive(Clone)]
pub struct UiTex {
    pub image: Handle<Image>,
    /// Size in texels of the disc's version of the page; sprite sheets and
    /// layout work in these.
    pub size: Vec2,
    /// Image pixels per texel: above 1 when the page came from an HD pack.
    pub px: f32,
}

impl UiTex {
    /// The whole page as a node of `w` x `h` units.
    pub fn node(&self, w: f32, h: f32) -> (Node, ImageNode) {
        (Node { width: px(w), height: px(h), ..default() }, ImageNode::new(self.image.clone()).with_mode(NodeImageMode::Stretch))
    }

    /// A texel region of the page, drawn `scale` units per texel.
    pub fn region(&self, rect: Rect, scale: f32) -> (Node, ImageNode) {
        let size = rect.size() * scale;
        (
            Node { width: px(size.x), height: px(size.y), ..default() },
            ImageNode { image: self.image.clone(), rect: Some(self.image_rect(rect)), image_mode: NodeImageMode::Stretch, ..default() },
        )
    }

    /// A texel region in the image's own pixels.
    pub fn image_rect(&self, rect: Rect) -> Rect {
        Rect { min: rect.min * self.px, max: rect.max * self.px }
    }
}

/// A sprite in texel units of its page.
#[derive(Clone, Copy, Debug)]
pub struct SheetSprite {
    pub rect: Rect,
    pub pivot: Option<Vec2>,
}

/// A texture page with its named sprites.
#[derive(Clone)]
pub struct Sheet {
    pub tex: UiTex,
    pub sprites: HashMap<String, SheetSprite>,
}

impl Sheet {
    pub fn get(&self, name: &str) -> SheetSprite {
        self.sprites.get(&name.to_ascii_lowercase()).copied().unwrap_or(SheetSprite { rect: Rect::new(0.0, 0.0, 1.0, 1.0), pivot: None })
    }

    /// A sprite as a node, `scale` units per texel.
    pub fn node(&self, name: &str, scale: f32) -> (Node, ImageNode) {
        self.tex.region(self.get(name).rect, scale)
    }
}

#[derive(Clone, Copy)]
struct Glyph {
    rect: Rect,
    /// Baseline origin, in texels.
    pivot: Vec2,
}

/// One of the disc's bitmap fonts.
#[derive(Clone)]
pub struct Font {
    tex: UiTex,
    glyphs: HashMap<char, Glyph>,
    /// Height of a capital letter cell, in texels.
    em: f32,
    /// Tallest cell top above the baseline, and deepest bottom below it.
    ascent: f32,
    descent: f32,
}

impl Font {
    fn glyph(&self, c: char) -> Option<Glyph> {
        self.glyphs.get(&c).or_else(|| self.glyphs.get(&c.to_ascii_uppercase())).copied()
    }

    /// Lays `text` out at a capital height of `size` units: glyph rects
    /// (texels) and their placement (units), plus the total size.
    fn layout(&self, text: &str, size: f32) -> (Vec<(Rect, Vec2, Vec2)>, Vec2) {
        let s = size / self.em;
        let mut pen = 0.0;
        let mut out = Vec::new();
        for c in text.chars() {
            match self.glyph(c) {
                Some(g) => {
                    let top = (self.ascent - (g.pivot.y - g.rect.min.y)) * s;
                    out.push((g.rect, Vec2::new(pen, top), g.rect.size() * s));
                    pen += g.rect.width() * s;
                }
                None => pen += self.em * 0.4 * s,
            }
        }
        (out, Vec2::new(pen, (self.ascent + self.descent) * s))
    }

    pub fn width(&self, text: &str, size: f32) -> f32 {
        self.layout(text, size).1.x
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FontId {
    /// White outlined capitals: titles and menus (`MgrammeCombined`).
    #[default]
    Menu,
    /// Small white capitals (`MgrammeSmall`).
    Small,
    /// Chrome mixed-case font (`kim36`): HUD figures.
    Chrome,
    /// The fill and the outline of `Menu`'s capitals (`MgrammeInside`,
    /// `MgrammeOutline`), for text in two colours (see [`two_tone`]).
    Inside,
    Outline,
}

/// A line of text in one of the game's fonts. The node's size follows the
/// text; position it with the usual `Node` fields.
#[derive(Component, Clone, Default)]
#[require(Node)]
pub struct BitmapText {
    pub text: String,
    pub font: FontId,
    /// Capital height in screen units.
    pub size: f32,
    pub color: Color,
}

impl BitmapText {
    pub fn new(font: FontId, size: f32, text: impl Into<String>) -> Self {
        Self { text: text.into(), font, size, color: Color::WHITE }
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
}

/// Text drawn as the original draws its big text (0x47910): the outline
/// font in one colour, then the fill font over it in another. The fonts
/// share their layout, so the fill sits on the outline.
pub fn two_tone(size: f32, text: impl Into<String>, fill: Color, outline: Color) -> impl Bundle {
    let text = text.into();
    (
        Node::default(),
        children![
            BitmapText::new(FontId::Outline, size, text.clone()).color(outline),
            (BitmapText::new(FontId::Inside, size, text).color(fill), Node { position_type: PositionType::Absolute, left: px(0), top: px(0), ..default() }),
        ],
    )
}

/// Changes a text only when it differs, so unchanged text is not laid out
/// again every frame.
pub fn set_text(t: &mut Mut<BitmapText>, s: &str) {
    if t.text != s {
        t.text = s.to_string();
    }
}

#[derive(Component)]
struct GlyphNode;

fn layout_bitmap_text(
    mut commands: Commands,
    assets: Option<Res<UiAssets>>,
    mut texts: Query<(Entity, &BitmapText, &mut Node, Option<&Children>), Changed<BitmapText>>,
    glyphs: Query<(), With<GlyphNode>>,
) {
    let Some(assets) = assets else { return };
    for (e, t, mut node, children) in &mut texts {
        for &c in children.into_iter().flatten() {
            if glyphs.contains(c) {
                commands.entity(c).despawn();
            }
        }
        let font = assets.font(t.font);
        let (glyph_list, size) = font.layout(&t.text, t.size);
        node.width = px(size.x);
        node.height = px(size.y);
        commands.entity(e).with_children(|p| {
            for (rect, at, sz) in glyph_list {
                p.spawn((
                    GlyphNode,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(at.x),
                        top: px(at.y),
                        width: px(sz.x),
                        height: px(sz.y),
                        ..default()
                    },
                    ImageNode { image: font.tex.image.clone(), rect: Some(font.tex.image_rect(rect)), color: t.color, image_mode: NodeImageMode::Stretch, ..default() },
                ));
            }
        });
    }
}

/// Everything 2D the HUD and the front end draw, loaded once at start-up.
#[derive(Resource, Clone)]
pub struct UiAssets {
    pub hud: Sheet,
    /// Preload arc revealed in steps, for the gauge's preload meter.
    pub preload_steps: Vec<Handle<Image>>,
    /// The speed boost disc revealed clockwise from 12 o'clock in steps,
    /// as the original fills it (0xa2a00).
    pub boost_steps: Vec<Handle<Image>>,
    pub positions: Sheet,
    pub countdown: Sheet,
    /// `Position_meter`: track progress bar, markers and the checkpoint
    /// banner (no sheet on the disc; regions measured from the page).
    pub meter: Sheet,
    pub design: Sheet,
    /// The front end's button pictures for Xbox, PlayStation and
    /// Nintendo pads (the disc's Xbox, PS2 and GameCube sheets); see
    /// [`UiAssets::button`].
    pub pad: Sheet,
    pub pad_ps: Sheet,
    pub pad_gc: Sheet,
    pub bio: Sheet,
    /// Options: the volume bar (`BAR_ON`, `BAR_OFF`), the tick box and the
    /// big Xbox pad picture.
    pub sound_bar: Sheet,
    pub checkbox: Sheet,
    /// The Trick Book's pad picture (`PAD`), the spots of its buttons
    /// (`BUTTON0`-`BUTTON9`) and the glows that light them; the PS2's
    /// and GameCube's for those pads (see [`UiAssets::trick_pad_for`]).
    pub trick_pad: Sheet,
    pub trick_pad_ps: Sheet,
    pub trick_pad_gc: Sheet,
    /// Point Distribution's bars (`HBAR8`, `ONEBAR`) and slider (`BLOB1`).
    pub points: Sheet,
    /// Name Your Rider's keyboard panel (`NAMEPANEL`) and name box
    /// (`SMALLNAMEPANEL`).
    pub name_entry: Sheet,
    /// The HUD's medal list icons (`GOLDMEDAL`, `SILVERMEDAL`,
    /// `BRONZEMEDAL`).
    pub small_medals: Sheet,
    /// Profile Select's numbers 1 to 10, eight to a row of 64-texel tiles.
    pub profile_numbers: UiTex,
    pub logo: UiTex,
    pub mugs: UiTex,
    pub loading: UiTex,
    /// The race loading screen's bands, an 8x256 strip stretched over the
    /// track's picture: navy at the top and bottom, see-through between,
    /// then light and dark grey ones for the racing tip.
    pub loading_overlay: UiTex,
    /// The results page's bands (`ResultsOverlay`), stretched the same way
    /// over the race.
    pub results_overlay: UiTex,
    /// Time Trial's results bands (`ResultsOverlay2`): the same without the
    /// band behind the race bests.
    pub results_overlay2: UiTex,
    pub backgrounds: HashMap<String, UiTex>,
    pub tracks: HashMap<String, UiTex>,
    /// The Freestyle Challenges' pictures, by number.
    pub challenges: HashMap<usize, UiTex>,
    fonts: [Font; 5],
}

impl UiAssets {
    pub fn font(&self, id: FontId) -> &Font {
        &self.fonts[id as usize]
    }

    /// The picture of `b` on a `style` pad: its sheet and sprite. The
    /// GameCube sheet's letters stand for a Nintendo pad's, by label
    /// (Bevy's buttons go by place, so South is Nintendo's B). Buttons a
    /// sheet lacks come from the Xbox one.
    pub fn button(&self, style: PadStyle, b: GamepadButton) -> (&Sheet, &'static str) {
        use GamepadButton as B;
        let xbox = match b {
            B::South => "controlbuttoncross",
            B::East => "controlbuttoncircle",
            B::West => "controlbuttonsquare",
            B::North => "YButton",
            B::Select => "controlbuttontriangle",
            _ => "StartButton",
        };
        let other = match (style, b) {
            (PadStyle::PlayStation, B::South | B::East | B::West) => Some((&self.pad_ps, xbox)),
            (PadStyle::PlayStation, B::North) => Some((&self.pad_ps, "controlbuttontriangle")),
            (PadStyle::Nintendo, B::South) => Some((&self.pad_gc, "controlbuttontriangle")),
            (PadStyle::Nintendo, B::East) => Some((&self.pad_gc, "controlbuttoncross")),
            (PadStyle::Nintendo, B::West) => Some((&self.pad_gc, "controlbuttonsquare")),
            (PadStyle::Nintendo, B::North) => Some((&self.pad_gc, "controlbuttoncircle")),
            (PadStyle::Nintendo, B::Start) => Some((&self.pad_gc, "StartButton")),
            _ => None,
        };
        other.unwrap_or((&self.pad, xbox))
    }

    /// [`UiAssets::button`] as a node `height` units tall.
    pub fn button_node(&self, style: PadStyle, b: GamepadButton, height: f32) -> (Node, ImageNode) {
        let (sheet, name) = self.button(style, b);
        let s = sheet.get(name).rect;
        sheet.tex.region(s, height / s.height())
    }

    /// The Trick Book's pad picture for a `style` pad.
    pub fn trick_pad_for(&self, style: PadStyle) -> &Sheet {
        match style {
            PadStyle::Xbox => &self.trick_pad,
            PadStyle::PlayStation => &self.trick_pad_ps,
            PadStyle::Nintendo => &self.trick_pad_gc,
        }
    }

    pub fn background(&self, name: &str) -> Option<&UiTex> {
        self.backgrounds.get(&name.to_ascii_lowercase())
    }

    pub fn load(game: &GameSource, images: &mut Assets<Image>) -> Result<Self> {
        let mut l = PageLoader { game, images };
        let hud = l.sheet("Sprites/HUDTPAGE")?;
        let preload_steps = l.preload_steps(&hud)?;
        let boost_steps = l.boost_steps(&hud)?;
        let meter_tex = l.tex("Sprites/Position_meter")?;
        let meter = Sheet {
            tex: meter_tex,
            sprites: [
                ("start", [155, 38, 188, 68]),
                ("line", [102, 39, 110, 503]),
                ("bar", [7, 42, 91, 470]),
                ("marker", [152, 106, 191, 130]),
                ("flag", [155, 270, 191, 305]),
                ("checkpoint", [125, 332, 240, 352]),
                ("pin", [160, 379, 195, 400]),
            ]
            .into_iter()
            .map(|(n, [x0, y0, x1, y1])| (n.to_string(), SheetSprite { rect: Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32), pivot: None }))
            .collect(),
        };
        let mut backgrounds = HashMap::new();
        let mut tracks = HashMap::new();
        let mut challenges = HashMap::new();
        let paths: Vec<String> = game.archive_paths().map(str::to_owned).collect();
        for p in &paths {
            let lower = p.to_ascii_lowercase();
            let Some(stem) = lower.strip_suffix(".bog") else { continue };
            if let Some(name) = stem.strip_prefix("/frontend/bg/") {
                backgrounds.insert(name.to_string(), l.tex(stem)?);
            } else if let Some(name) = stem.strip_prefix("/frontend/track_").and_then(|s| s.strip_suffix("_large")) {
                tracks.insert(name.to_string(), l.tex(stem)?);
            } else if let Some(n) = stem.strip_prefix("/frontend/freestyle_challenge").and_then(|s| s.strip_suffix("_small")).and_then(|n| n.parse().ok()) {
                challenges.insert(n, l.tex(stem)?);
            }
        }
        Ok(Self {
            preload_steps,
            boost_steps,
            positions: l.sheet("Sprites/positions")?,
            countdown: l.sheet("Sprites/321go")?,
            meter,
            design: l.sheet("FrontEnd/NewDesign")?,
            pad: l.sheet("FrontEnd/controlpadbuttonsXbox")?,
            pad_ps: l.sheet_on_black("FrontEnd/controlpadbuttonsPS2")?,
            pad_gc: l.sheet("FrontEnd/controlpadbuttonsGC")?,
            bio: l.sheet("FrontEnd/BioPanel")?,
            sound_bar: l.sheet("FrontEnd/SoundBar")?,
            checkbox: l.sheet("FrontEnd/checkbox")?,
            trick_pad: l.sheet("FrontEnd/TrickPad_XBox")?,
            trick_pad_ps: l.sheet("FrontEnd/TrickPad_PS2")?,
            trick_pad_gc: l.sheet("FrontEnd/TrickPad_GC")?,
            points: l.sheet("FrontEnd/PointsTpage")?,
            name_entry: l.sheet("FrontEnd/nameentrypanel")?,
            small_medals: l.sheet("FrontEnd/SmallMedals")?,
            profile_numbers: l.tex("FrontEnd/ProfileNumbers")?,
            logo: l.tex("FrontEnd/ATVLogoBIG")?,
            mugs: l.tex("FrontEnd/RiderMugs")?,
            loading: l.tex("FrontEnd/FELoading1")?,
            loading_overlay: l.tex("FrontEnd/LoadingOverlay")?,
            results_overlay: l.tex("FrontEnd/ResultsOverlay")?,
            results_overlay2: l.tex("FrontEnd/ResultsOverlay2")?,
            backgrounds,
            tracks,
            challenges,
            fonts: [
                l.font("Fonts/MgrammeCombined")?,
                l.font("Fonts/MgrammeSmall")?,
                l.font("Fonts/kim36")?,
                l.font("Fonts/MgrammeInside")?,
                l.font("Fonts/Mgrammeoutline")?,
            ],
            hud,
        })
    }
}

struct PageLoader<'a> {
    game: &'a GameSource,
    images: &'a mut Assets<Image>,
}

impl PageLoader<'_> {
    /// A page's pixels and its image pixels per texel (see [`UiTex::px`]).
    fn pixels(&self, path: &str) -> Result<(u32, u32, Vec<u8>, f32)> {
        let path = format!("/{}.bog", path.trim_start_matches('/'));
        let (tex, scale) = crate::hd::texture(self.game, &path)?;
        Ok((tex.width, tex.height, tex.to_rgba8(), scale))
    }

    fn add(&mut self, w: u32, h: u32, rgba: Vec<u8>) -> UiTex {
        let image = Image::new(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        UiTex { image: self.images.add(image), size: Vec2::new(w as f32, h as f32), px: 1.0 }
    }

    fn tex(&mut self, path: &str) -> Result<UiTex> {
        let (w, h, px, scale) = self.pixels(path)?;
        let tex = self.add(w, h, px);
        Ok(UiTex { size: tex.size / scale, px: scale, ..tex })
    }

    fn bed(&self, path: &str) -> Result<SpriteSheet> {
        let path = format!("/{}.bed", path.trim_start_matches('/'));
        Ok(SpriteSheet::parse(&self.game.read(&path).map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?)?)
    }

    fn sheet(&mut self, path: &str) -> Result<Sheet> {
        let tex = self.tex(path)?;
        self.with_sprites(path, tex)
    }

    fn with_sprites(&self, path: &str, tex: UiTex) -> Result<Sheet> {
        let bed = self.bed(path)?;
        let sprites = bed.sprites().map(|(n, s)| (n.to_ascii_lowercase(), to_texels(&s, tex.size))).collect();
        Ok(Sheet { tex, sprites })
    }

    /// A sheet drawn on black with no alpha (the PS2 button pictures, a
    /// DXT1 page): the black faded out, so the buttons sit on the page.
    fn sheet_on_black(&mut self, path: &str) -> Result<Sheet> {
        let (w, h, mut px, scale) = self.pixels(path)?;
        for p in px.as_chunks_mut::<4>().0 {
            let bright = p[0].max(p[1]).max(p[2]);
            p[3] = p[3].min((u32::from(bright) * 255 / 40).min(255) as u8);
        }
        let tex = self.add(w, h, px);
        self.with_sprites(path, UiTex { size: tex.size / scale, px: scale, ..tex })
    }

    fn font(&mut self, path: &str) -> Result<Font> {
        let tex = self.tex(path)?;
        let bed = self.bed(path)?;
        let glyphs: HashMap<char, Glyph> = bed
            .glyphs()
            .map(|(c, s)| {
                let t = to_texels(&s, tex.size);
                (c, Glyph { rect: t.rect, pivot: t.pivot.unwrap_or(Vec2::new(t.rect.min.x, t.rect.max.y)) })
            })
            .collect();
        let em = glyphs.get(&'A').or_else(|| glyphs.get(&'0')).map(|g| g.pivot.y - g.rect.min.y).unwrap_or(32.0);
        let ascent = glyphs.values().map(|g| g.pivot.y - g.rect.min.y).fold(0.0, f32::max);
        let descent = glyphs.values().map(|g| g.rect.max.y - g.pivot.y).fold(0.0, f32::max);
        Ok(Font { tex, glyphs, em, ascent, descent })
    }

    /// The gauge's preload meter fills along its arc. The arc sprite is cut
    /// into angular steps around the gauge hub, from its blue end (bottom)
    /// round to its red end (top right).
    fn boost_steps(&mut self, hud: &Sheet) -> Result<Vec<Handle<Image>>> {
        const STEPS: usize = 32;
        let (w, _h, px, k) = self.pixels("Sprites/HUDTPAGE")?;
        let disc = hud.get("BOOST").rect;
        let (x0, y0) = ((disc.min.x * k) as u32, (disc.min.y * k) as u32);
        let (dw, dh) = ((disc.width() * k) as u32, (disc.height() * k) as u32);
        // Clockwise from straight up, in the square the pie is cut from.
        let turn = |x: u32, y: u32| {
            let (u, v) = ((x as f32 + 0.5) / dw as f32 * 2.0 - 1.0, (y as f32 + 0.5) / dh as f32 * 2.0 - 1.0);
            u.atan2(-v).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
        };
        let mut steps = Vec::with_capacity(STEPS);
        for n in 1..=STEPS {
            let limit = n as f32 / STEPS as f32;
            let mut out = vec![0u8; (dw * dh * 4) as usize];
            for y in 0..dh {
                for x in 0..dw {
                    if turn(x, y) <= limit {
                        let src = (((y0 + y) * w + x0 + x) * 4) as usize;
                        let dst = ((y * dw + x) * 4) as usize;
                        out[dst..dst + 4].copy_from_slice(&px[src..src + 4]);
                    }
                }
            }
            steps.push(self.add(dw, dh, out).image);
        }
        Ok(steps)
    }

    fn preload_steps(&mut self, hud: &Sheet) -> Result<Vec<Handle<Image>>> {
        const STEPS: usize = 24;
        let (w, _h, px, k) = self.pixels("Sprites/HUDTPAGE")?;
        // Worked in image pixels, which are texels unless the page is HD.
        let arc = hud.get("PRELOAD").rect;
        let arc = Rect { min: arc.min * k, max: arc.max * k };
        let slot = hud.get("DIAL_PRELOAD").rect;
        let dial = hud.get("DIAL");
        let hub = (dial.pivot.unwrap_or(dial.rect.center()) - slot.min) * k;
        let (x0, y0) = (arc.min.x as u32, arc.min.y as u32);
        let (aw, ah) = (arc.width() as u32, arc.height() as u32);
        // Clockwise angle from straight up, around the hub.
        let angle = |x: u32, y: u32| (x as f32 + 0.5 - hub.x).atan2(-(y as f32 + 0.5 - hub.y));
        let mut lit = Vec::new();
        for y in 0..ah {
            for x in 0..aw {
                if px[(((y0 + y) * w + x0 + x) * 4 + 3) as usize] > 8 {
                    lit.push(angle(x, y));
                }
            }
        }
        let lo = lit.iter().copied().fold(f32::MAX, f32::min);
        let hi = lit.iter().copied().fold(f32::MIN, f32::max);
        let mut steps = Vec::with_capacity(STEPS);
        for k in 1..=STEPS {
            let limit = lo + (hi - lo) * k as f32 / STEPS as f32;
            let mut out = vec![0u8; (aw * ah * 4) as usize];
            for y in 0..ah {
                for x in 0..aw {
                    if angle(x, y) <= limit {
                        let src = (((y0 + y) * w + x0 + x) * 4) as usize;
                        let dst = ((y * aw + x) * 4) as usize;
                        out[dst..dst + 4].copy_from_slice(&px[src..src + 4]);
                    }
                }
            }
            steps.push(self.add(aw, ah, out).image);
        }
        Ok(steps)
    }
}

fn to_texels(s: &Sprite, size: Vec2) -> SheetSprite {
    let p = |v: [f32; 2]| Vec2::new(v[0] * size.x, v[1] * size.y);
    SheetSprite { rect: Rect::from_corners(p(s.min), p(s.max)), pivot: s.pivot.map(p) }
}

/// A full-screen layer for one screen or overlay.
pub fn fullscreen() -> Node {
    Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }
}

/// Gives a sprite node an absolute position, in screen units.
pub fn placed((mut node, image): (Node, ImageNode), left: f32, top: f32) -> (Node, ImageNode) {
    node.position_type = PositionType::Absolute;
    node.left = px(left);
    node.top = px(top);
    (node, image)
}

/// One frame's menu input from the keyboard and any gamepad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Accept,
    /// Escape, Backspace or B.
    Back,
    /// The gamepad's Start button.
    Start,
    /// Y on a pad, or T: the Trick Book where a page offers it.
    Extra,
}

pub fn read_nav(keys: &ButtonInput<KeyCode>, pads: &Query<&bevy::input::gamepad::Gamepad>) -> Option<Nav> {
    read_nav_from(keys, pads.iter())
}

/// [`read_nav`] taking pad input from `pads` only (in split screen, a
/// player's own pad); the keyboard always counts.
pub fn read_nav_from<'a>(keys: &ButtonInput<KeyCode>, pads: impl IntoIterator<Item = &'a bevy::input::gamepad::Gamepad>) -> Option<Nav> {
    use bevy::input::gamepad::GamepadButton as B;
    let pads: Vec<_> = pads.into_iter().collect();
    let k = |c: &[KeyCode]| c.iter().any(|c| keys.just_pressed(*c));
    let p = |b: B| pads.iter().any(|p| p.just_pressed(b));
    if k(&[KeyCode::ArrowUp, KeyCode::KeyW]) || p(B::DPadUp) {
        Some(Nav::Up)
    } else if k(&[KeyCode::ArrowDown, KeyCode::KeyS]) || p(B::DPadDown) {
        Some(Nav::Down)
    } else if k(&[KeyCode::ArrowLeft, KeyCode::KeyA]) || p(B::DPadLeft) {
        Some(Nav::Left)
    } else if k(&[KeyCode::ArrowRight, KeyCode::KeyD]) || p(B::DPadRight) {
        Some(Nav::Right)
    } else if k(&[KeyCode::Enter, KeyCode::NumpadEnter, KeyCode::Space]) || p(B::South) {
        Some(Nav::Accept)
    } else if k(&[KeyCode::Backspace, KeyCode::Escape]) || p(B::East) {
        Some(Nav::Back)
    } else if p(B::Start) {
        Some(Nav::Start)
    } else if k(&[KeyCode::KeyT]) || p(B::North) {
        Some(Nav::Extra)
    } else {
        None
    }
}

/// A vertical list of menu items with the game's selection bar behind the
/// chosen one. Screens move `selected`; the highlight follows.
#[derive(Component, Clone)]
pub struct MenuList {
    pub items: Vec<String>,
    /// Items shown dimmed and skipped over (modes not built yet).
    pub disabled: Vec<bool>,
    /// Items shown dimmed that can be highlighted but not picked (locked
    /// tracks and ATVs).
    pub locked: Vec<bool>,
    pub selected: usize,
    pub width: f32,
    pub row: f32,
}

impl MenuList {
    pub fn new(items: &[&str], width: f32) -> Self {
        Self { items: items.iter().map(|s| s.to_string()).collect(), disabled: vec![false; items.len()], locked: vec![false; items.len()], selected: 0, width, row: 34.0 }
    }

    pub fn with_row(mut self, row: f32) -> Self {
        self.row = row;
        self
    }

    pub fn select(mut self, i: usize) -> Self {
        self.selected = i.min(self.items.len().saturating_sub(1));
        self
    }

    pub fn disable(mut self, items: &[&str]) -> Self {
        for (i, it) in self.items.iter().enumerate() {
            self.disabled[i] = items.contains(&it.as_str());
        }
        if self.disabled.get(self.selected) == Some(&true) {
            self.step(Nav::Down);
        }
        self
    }

    /// Marks items locked by index.
    pub fn lock(mut self, locked: impl Fn(usize) -> bool) -> Self {
        self.locked = (0..self.items.len()).map(locked).collect();
        self
    }

    /// Whether the highlighted item can't be picked.
    pub fn blocked(&self) -> bool {
        [&self.disabled, &self.locked].iter().any(|v| v.get(self.selected) == Some(&true))
    }

    pub fn step(&mut self, nav: Nav) {
        let n = self.items.len().max(1);
        for _ in 0..n {
            match nav {
                Nav::Up => self.selected = (self.selected + n - 1) % n,
                Nav::Down => self.selected = (self.selected + 1) % n,
                _ => return,
            }
            if !self.disabled.get(self.selected).copied().unwrap_or(false) {
                return;
            }
        }
    }

    pub fn current(&self) -> &str {
        self.items.get(self.selected).map_or("", String::as_str)
    }
}

#[derive(Component)]
struct MenuRow(usize);

/// Builds a menu's rows; call once after spawning a [`MenuList`].
pub fn spawn_menu_rows(commands: &mut Commands, assets: &UiAssets, menu: Entity, list: &MenuList) {
    commands.entity(menu).insert(Node { flex_direction: FlexDirection::Column, ..default() });
    commands.entity(menu).with_children(|p| {
        for (i, item) in list.items.iter().enumerate() {
            let (mut bar, img) = assets.design.node("MENU_SELECT", 1.0);
            bar.position_type = PositionType::Absolute;
            bar.width = percent(100);
            bar.height = percent(100);
            p.spawn(Node {
                width: px(list.width),
                height: px(list.row),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|row| {
                row.spawn((bar, img, MenuRow(i), Visibility::Hidden));
                row.spawn(BitmapText::new(FontId::Menu, (list.row * 0.44).min(15.0), item.clone()));
            });
        }
    });
}

fn refresh_menus(lists: Query<(&MenuList, &Children)>, rows: Query<&Children>, mut bars: Query<(&MenuRow, &mut Visibility)>, mut texts: Query<&mut BitmapText>) {
    for (list, children) in &lists {
        for &row in children {
            let Ok(parts) = rows.get(row) else { continue };
            for &part in parts {
                if let Ok((r, mut vis)) = bars.get_mut(part) {
                    let on = r.0 == list.selected;
                    vis.set_if_neq(if on { Visibility::Inherited } else { Visibility::Hidden });
                    for &other in parts {
                        if let Ok(mut t) = texts.get_mut(other) {
                            let c = if list.disabled.get(r.0) == Some(&true) {
                                Color::srgba(0.6, 0.65, 0.75, 0.45)
                            } else if list.locked.get(r.0) == Some(&true) {
                                Color::srgba(0.6, 0.65, 0.75, if on { 0.8 } else { 0.45 })
                            } else if on {
                                Color::WHITE
                            } else {
                                Color::srgb(0.72, 0.78, 0.9)
                            };
                            if t.color != c {
                                t.color = c;
                            }
                        }
                    }
                }
            }
        }
    }
}
