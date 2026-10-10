//! The game's own 2D art: texture pages and their `.bed` sprite sheets, and
//! the bitmap fonts, all read from the player's disc and drawn with Bevy UI.
//!
//! Layout works in the original's 640x480 screen units: [`UiScale`] maps
//! 480 units to the window height, so the HUD and menus keep their size on
//! any resolution and widescreen just adds room at the sides.

use std::collections::HashMap;

use anyhow::Result;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::widget::NodeImageMode;
use bevy::window::PrimaryWindow;

use atv_formats::bed::{Sprite, SpriteSheet};
use atv_formats::GameSource;

/// Screen height the original UI was laid out for.
pub const SCREEN_H: f32 = 480.0;
pub const SCREEN_W: f32 = 640.0;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (fit_ui_scale, refresh_menus, layout_bitmap_text).chain().before(bevy::ui::UiSystems::Prepare));
    }
}

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
    pub positions: Sheet,
    pub countdown: Sheet,
    /// `Position_meter`: track progress bar, markers and the checkpoint
    /// banner (no sheet on the disc; regions measured from the page).
    pub meter: Sheet,
    pub design: Sheet,
    pub pad: Sheet,
    pub bio: Sheet,
    pub logo: UiTex,
    pub mugs: UiTex,
    pub loading: UiTex,
    pub backgrounds: HashMap<String, UiTex>,
    pub tracks: HashMap<String, UiTex>,
    fonts: [Font; 3],
}

impl UiAssets {
    pub fn font(&self, id: FontId) -> &Font {
        &self.fonts[id as usize]
    }

    pub fn background(&self, name: &str) -> Option<&UiTex> {
        self.backgrounds.get(&name.to_ascii_lowercase())
    }

    pub fn load(game: &GameSource, images: &mut Assets<Image>) -> Result<Self> {
        let mut l = PageLoader { game, images };
        let hud = l.sheet("Sprites/HUDTPAGE")?;
        let preload_steps = l.preload_steps(&hud)?;
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
        let paths: Vec<String> = game.archive_paths().map(str::to_owned).collect();
        for p in &paths {
            let lower = p.to_ascii_lowercase();
            let Some(stem) = lower.strip_suffix(".bog") else { continue };
            if let Some(name) = stem.strip_prefix("/frontend/bg/") {
                backgrounds.insert(name.to_string(), l.tex(stem)?);
            } else if let Some(name) = stem.strip_prefix("/frontend/track_").and_then(|s| s.strip_suffix("_large")) {
                tracks.insert(name.to_string(), l.tex(stem)?);
            }
        }
        Ok(Self {
            preload_steps,
            positions: l.sheet("Sprites/positions")?,
            countdown: l.sheet("Sprites/321go")?,
            meter,
            design: l.sheet("FrontEnd/NewDesign")?,
            pad: l.sheet("FrontEnd/controlpadbuttonsXbox")?,
            bio: l.sheet("FrontEnd/BioPanel")?,
            logo: l.tex("FrontEnd/ATVLogoBIG")?,
            mugs: l.tex("FrontEnd/RiderMugs")?,
            loading: l.tex("FrontEnd/FELoading1")?,
            backgrounds,
            tracks,
            fonts: [l.font("Fonts/MgrammeCombined")?, l.font("Fonts/MgrammeSmall")?, l.font("Fonts/kim36")?],
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
        let bed = self.bed(path)?;
        let sprites = bed.sprites().map(|(n, s)| (n.to_ascii_lowercase(), to_texels(&s, tex.size))).collect();
        Ok(Sheet { tex, sprites })
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
}

pub fn read_nav(keys: &ButtonInput<KeyCode>, pads: &Query<&bevy::input::gamepad::Gamepad>) -> Option<Nav> {
    use bevy::input::gamepad::GamepadButton as B;
    let k = |c: &[KeyCode]| c.iter().any(|c| keys.just_pressed(*c));
    let p = |b: B| pads.iter().any(|pad| pad.just_pressed(b));
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
    pub selected: usize,
    pub width: f32,
    pub row: f32,
}

impl MenuList {
    pub fn new(items: &[&str], width: f32) -> Self {
        Self { items: items.iter().map(|s| s.to_string()).collect(), disabled: vec![false; items.len()], selected: 0, width, row: 34.0 }
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
