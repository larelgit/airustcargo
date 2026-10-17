//! Screens and the theme as content: `content/**/*.screen.ron` and `content/**/*.theme.ron`.
//!
//! ```ron
//! // content/ui/pause.screen.ron
//! {
//!     "pause": (
//!         pauses: true,
//!         title: "Paused",
//!         widgets: [
//!             Button(id: "resume", label: "Resume", action: "close"),
//!             Button(id: "settings", label: "Settings", action: "open settings"),
//!             Button(id: "quit", label: "Quit", action: "quit"),
//!         ],
//!     ),
//! }
//! ```
//!
//! Text a player reads (a title, a label, a text, an option's label, a stat) can name a message
//! of the game's languages instead: `label: "@menu-resume"` shows that message in the language
//! played, and again in another when the player switches. `@@` starts a text with `@`. The
//! content check finds a message no language has.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};

/// Where a screen sits and whether it takes the controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Layer {
    /// Shown while playing and never takes input: bars, hints, the compass.
    Hud,
    /// A menu: takes the controls while it is open (pause, settings, inventory).
    #[default]
    Menu,
    /// Over every menu: a question that must be answered, a death screen.
    Modal,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Hud => "hud",
            Layer::Menu => "menu",
            Layer::Modal => "modal",
        }
    }
}

/// Where on the screen a screen's widgets are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

/// How a container lays out its children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    /// One under another: `up` and `down` walk them.
    #[default]
    Column,
    /// Side by side: `left` and `right` walk them.
    Row,
}

/// A screen (kind `screen`): what it holds, where it sits, how it behaves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenDef {
    #[serde(default)]
    pub layer: Layer,
    /// In single player the game pauses while it is open (`PauseState::Paused`); in co-op the
    /// world goes on.
    #[serde(default)]
    pub pauses: bool,
    /// Opened when the game starts (`OnEnter(EngineState::InGame)`): a HUD.
    #[serde(default)]
    pub open_at_start: bool,
    /// `back` closes it. A screen that must be answered says `false`, and closes from a button.
    #[serde(default = "yes")]
    pub closable: bool,
    /// Shown on top of it, and what a screen reader calls it.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub anchor: Anchor,
    #[serde(default)]
    pub layout: Layout,
    pub widgets: Vec<WidgetDef>,
}

fn yes() -> bool {
    true
}

fn one() -> f32 {
    1.0
}

/// One widget. Ids are unique within a screen, and an agent names a widget `screen/id`
/// (`pause/resume`); a widget without one gets its kind and number (`text2`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WidgetDef {
    /// Widgets one under another, or side by side, with an optional title.
    Panel {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        layout: Layout,
        children: Vec<WidgetDef>,
    },
    /// Items one under another (a screen reader says "list").
    List {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        title: Option<String>,
        children: Vec<WidgetDef>,
    },
    /// Items in rows of `columns`: the arrows walk it both ways.
    Grid {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        title: Option<String>,
        columns: u32,
        children: Vec<WidgetDef>,
    },
    /// Pages, one shown at a time: each child is a page and its `title` the tab's name.
    /// `left`/`right` on the tabs (or `ui_set`) turn the page.
    Tabs { id: String, children: Vec<WidgetDef> },
    /// Text, or what source `bind` says.
    Text {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        text: String,
        #[serde(default)]
        bind: Option<String>,
    },
    /// A hint (`[E] Open the gate`): hidden while its text, or its source's, is empty.
    Hint {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        text: String,
        #[serde(default)]
        bind: Option<String>,
    },
    /// Does `action` when pressed: `close`, `back`, `open <screen>`, `quit`, `tool <name> {json}`
    /// or a name the game handles (`add_ui_action`).
    Button {
        id: String,
        label: String,
        action: String,
        #[serde(default = "yes")]
        enabled: bool,
    },
    /// A value out of a maximum (health, stamina, poise): from source `bind`, or fixed.
    Bar {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: String,
        #[serde(default)]
        bind: Option<String>,
        #[serde(default)]
        value: f32,
        #[serde(default = "one")]
        max: f32,
        /// `#rrggbb`, or a name in the theme's `colors` (`health`). Default: the accent.
        #[serde(default)]
        color: Option<String>,
    },
    /// An action waiting to be ready again: source `you.cooldown:<action>`.
    Cooldown {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: String,
        bind: String,
    },
    /// Which way the viewer faces, and marks around: source `you.heading`.
    Compass {
        #[serde(default)]
        id: Option<String>,
        #[serde(default = "heading")]
        bind: String,
    },
    /// The other players: name, health, down or not. Source `allies`.
    Allies {
        #[serde(default)]
        id: Option<String>,
        #[serde(default = "allies")]
        bind: String,
    },
    /// Stats before and after a change (`damage 12 → 15 (+3)`): fixed rows or source `bind`.
    Compare {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        rows: Vec<CompareRow>,
        #[serde(default)]
        bind: Option<String>,
    },
    /// A number between `min` and `max`, in steps of `step` (0: any). With `setting`
    /// (`accessibility.text_scale`) it shows and changes that player setting.
    Slider {
        id: String,
        label: String,
        min: f32,
        max: f32,
        #[serde(default)]
        step: f32,
        #[serde(default)]
        value: Option<f32>,
        #[serde(default)]
        setting: Option<String>,
    },
    /// On or off; with `setting`, that player setting.
    Toggle {
        id: String,
        label: String,
        #[serde(default)]
        on: bool,
        #[serde(default)]
        setting: Option<String>,
    },
    /// The input of an action of the controls (`jump`: `Space · Pad South`), which the player
    /// changes: confirming it waits for the next key, mouse button or gamepad button (Escape
    /// cancels), and that input becomes the binding, saved in the `controls` settings. An input
    /// another action of the context reads is refused, and the widget says which action holds it.
    /// The engine's source `input.bindings` gives the inputs.
    Binding {
        id: String,
        label: String,
        /// The input context (`gameplay`, `menu`, a game's own).
        #[serde(default = "gameplay", skip_serializing_if = "is_gameplay")]
        context: String,
        /// The action (`jump`, `light`, `use_item`).
        action: String,
        /// Only this slot (0: the primary input; one past the last adds one). Without it the widget
        /// shows every input of the action, and a new one replaces the first input of its kind of
        /// device (keyboard and mouse, or gamepad), or is added when the action has none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<usize>,
        /// What it says while it waits (`@controls-press`). Default: `Press a key or button...
        /// (Esc cancels)`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        /// The message that says an input is taken, with `$input` and `$action`
        /// (`@controls-taken`). Default: `<input> is already used by <action>`, in English.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        taken: Option<String>,
    },
    /// One of `options`; with `setting`, that player setting.
    Choice {
        id: String,
        label: String,
        options: Vec<String>,
        /// What each option says, in the order of `options` (`"@difficulty-hard"`, `"Русский"`).
        /// Empty: the options themselves.
        #[serde(default)]
        labels: Vec<String>,
        /// The option shown at first (default: the first), and while the setting is none of the
        /// options (a language setting left empty: the game's own).
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        setting: Option<String>,
    },
}

fn heading() -> String {
    "you.heading".to_owned()
}

fn gameplay() -> String {
    "gameplay".to_owned()
}

fn is_gameplay(context: &str) -> bool {
    context == "gameplay"
}

fn allies() -> String {
    "allies".to_owned()
}

/// One row of a [`WidgetDef::Compare`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompareRow {
    pub stat: String,
    pub before: f32,
    pub after: f32,
}

impl WidgetDef {
    /// The id it was given, if any.
    pub fn id(&self) -> Option<&str> {
        match self {
            WidgetDef::Tabs { id, .. }
            | WidgetDef::Button { id, .. }
            | WidgetDef::Slider { id, .. }
            | WidgetDef::Toggle { id, .. }
            | WidgetDef::Binding { id, .. }
            | WidgetDef::Choice { id, .. } => Some(id),
            WidgetDef::Panel { id, .. }
            | WidgetDef::List { id, .. }
            | WidgetDef::Grid { id, .. }
            | WidgetDef::Text { id, .. }
            | WidgetDef::Hint { id, .. }
            | WidgetDef::Bar { id, .. }
            | WidgetDef::Cooldown { id, .. }
            | WidgetDef::Compass { id, .. }
            | WidgetDef::Allies { id, .. }
            | WidgetDef::Compare { id, .. } => id.as_deref(),
        }
    }

    /// The kind as a word: `button`, the prefix of ids made up for widgets without one.
    pub fn kind(&self) -> &'static str {
        match self {
            WidgetDef::Panel { .. } => "panel",
            WidgetDef::List { .. } => "list",
            WidgetDef::Grid { .. } => "grid",
            WidgetDef::Tabs { .. } => "tabs",
            WidgetDef::Text { .. } => "text",
            WidgetDef::Hint { .. } => "hint",
            WidgetDef::Button { .. } => "button",
            WidgetDef::Bar { .. } => "bar",
            WidgetDef::Cooldown { .. } => "cooldown",
            WidgetDef::Compass { .. } => "compass",
            WidgetDef::Allies { .. } => "allies",
            WidgetDef::Compare { .. } => "compare",
            WidgetDef::Slider { .. } => "slider",
            WidgetDef::Toggle { .. } => "toggle",
            WidgetDef::Binding { .. } => "binding",
            WidgetDef::Choice { .. } => "choice",
        }
    }

    /// The widgets inside it.
    pub fn children(&self) -> &[WidgetDef] {
        match self {
            WidgetDef::Panel { children, .. }
            | WidgetDef::List { children, .. }
            | WidgetDef::Grid { children, .. }
            | WidgetDef::Tabs { children, .. } => children,
            _ => &[],
        }
    }

    /// The title of a container (a tab's name, for a page).
    pub fn title(&self) -> Option<&str> {
        match self {
            WidgetDef::Panel { title, .. } | WidgetDef::List { title, .. } | WidgetDef::Grid { title, .. } | WidgetDef::Compare { title, .. } => {
                title.as_deref()
            }
            _ => None,
        }
    }

    /// The text a player reads in it as content writes it (`@menu-resume` or itself), and the
    /// field each is in: titles, labels, texts, options' labels, stats.
    pub fn texts(&self) -> Vec<(&str, &'static str)> {
        let mut texts: Vec<(&str, &'static str)> = self.title().map(|t| (t, "title")).into_iter().collect();
        match self {
            WidgetDef::Text { text, .. } | WidgetDef::Hint { text, .. } => texts.push((text, "text")),
            WidgetDef::Button { label, .. }
            | WidgetDef::Bar { label, .. }
            | WidgetDef::Cooldown { label, .. }
            | WidgetDef::Slider { label, .. }
            | WidgetDef::Toggle { label, .. } => texts.push((label, "label")),
            WidgetDef::Choice { label, labels, .. } => {
                texts.push((label, "label"));
                texts.extend(labels.iter().map(|l| (l.as_str(), "labels")));
            }
            WidgetDef::Binding { label, prompt, taken, .. } => {
                texts.push((label, "label"));
                texts.extend(prompt.as_deref().map(|p| (p, "prompt")));
                texts.extend(taken.as_deref().map(|t| (t, "taken")));
            }
            WidgetDef::Compare { rows, .. } => texts.extend(rows.iter().map(|r| (r.stat.as_str(), "rows.stat"))),
            _ => {}
        }
        texts
    }
}

/// What a button does, read from its `action`.
#[derive(Clone, Debug, PartialEq)]
pub enum ButtonAction {
    /// Closes the screen the button is on.
    Close,
    /// What `back` does: closes the top screen.
    Back,
    /// Opens a screen on top.
    Open(String),
    /// Ends the game (`AppExit`).
    Quit,
    /// Calls a tool as the agent that pressed the button: `tool use_item {"item": "flask"}`.
    Tool(String, serde_json::Value),
    /// A name the game handles with `add_ui_action`.
    Game(String),
}

impl ButtonAction {
    pub fn parse(action: &str) -> Result<ButtonAction, String> {
        let action = action.trim();
        let (word, rest) = action.split_once(char::is_whitespace).map_or((action, ""), |(w, r)| (w, r.trim()));
        Ok(match word {
            "" => return Err("an empty action: say what the button does (`close`, `open settings`, `quit`, a game action)".to_owned()),
            "close" if rest.is_empty() => ButtonAction::Close,
            "back" if rest.is_empty() => ButtonAction::Back,
            "quit" if rest.is_empty() => ButtonAction::Quit,
            "open" if !rest.is_empty() && !rest.contains(char::is_whitespace) => ButtonAction::Open(rest.to_owned()),
            "open" => return Err(format!("`{action}`: `open` takes one screen (`open settings`)")),
            "tool" => {
                let (tool, args) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(t, a)| (t, a.trim()));
                if tool.is_empty() {
                    return Err(format!("`{action}`: `tool` takes a tool and its arguments as JSON (`tool use_item {{\"item\": \"flask\"}}`)"));
                }
                let args = if args.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(args).map_err(|e| format!("`{action}`: the arguments are not JSON: {e}"))?
                };
                ButtonAction::Tool(tool.to_owned(), args)
            }
            "close" | "back" | "quit" => return Err(format!("`{action}`: `{word}` takes nothing after it")),
            _ if rest.is_empty() => ButtonAction::Game(word.to_owned()),
            _ => return Err(format!("`{action}`: a game action is one word (`respawn`), handled with `add_ui_action`")),
        })
    }
}

/// Whether `id` is a widget id: letters, digits, `_` and `-`.
fn good_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl ContentDef for ScreenDef {
    const KIND: &'static str = "screen";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if !good_id(id) {
            refs.error(format!("a screen id is letters, digits, `_` and `-` (`{id}` is not): agents name widgets `{id}/<widget>`"));
        }
        if self.layer == Layer::Hud && self.pauses {
            refs.error("a HUD never pauses the game: `pauses: true` needs `layer: Menu` or `Modal`");
        }
        let mut seen = BTreeSet::new();
        let mut counters = BTreeMap::new();
        for widget in &self.widgets {
            check_widget(widget, refs, &mut seen, &mut counters);
        }
        // Every message named is in each language or its fallbacks.
        let mut texts: Vec<(&str, String)> = self.title.as_deref().map(|t| (t, "`title`".to_owned())).into_iter().collect();
        let mut counters = BTreeMap::new();
        for widget in &self.widgets {
            texts_of(widget, &mut counters, &mut texts);
        }
        shards_locale::check_messages(refs, texts);
    }
}

/// The texts of `widget` and the widgets inside it, each with where it is (`widget `resume` label`).
fn texts_of<'a>(widget: &'a WidgetDef, counters: &mut BTreeMap<&'static str, u32>, out: &mut Vec<(&'a str, String)>) {
    let id = assign_id(widget, counters);
    out.extend(widget.texts().into_iter().map(|(text, field)| (text, format!("widget `{id}` {field}"))));
    for child in widget.children() {
        texts_of(child, counters, out);
    }
}

fn check_widget(widget: &WidgetDef, refs: &mut Refs, seen: &mut BTreeSet<String>, counters: &mut BTreeMap<&'static str, u32>) {
    let id = assign_id(widget, counters);
    if !good_id(&id) {
        refs.error(format!("widget `{id}`: an id is letters, digits, `_` and `-`"));
    } else if !seen.insert(id.clone()) {
        refs.error(format!("widget `{id}` twice: ids are unique within a screen"));
    }
    match widget {
        WidgetDef::Button { action, .. } => match ButtonAction::parse(action) {
            Ok(ButtonAction::Open(screen)) => {
                refs.require::<ScreenDef>(&screen, &format!("{id}.action"));
            }
            Ok(_) => {}
            Err(e) => refs.error(format!("button `{id}`: {e}")),
        },
        WidgetDef::Grid { columns: 0, .. } => refs.error(format!("grid `{id}`: `columns` is at least 1")),
        WidgetDef::Tabs { children, .. } => {
            if children.is_empty() {
                refs.error(format!("tabs `{id}`: no pages; each child is a page"));
            }
            for page in children {
                if page.id().is_none() || !matches!(page, WidgetDef::Panel { .. } | WidgetDef::List { .. } | WidgetDef::Grid { .. }) {
                    refs.error(format!("tabs `{id}`: a page is a Panel, List or Grid with an `id` (and a `title` for its tab)"));
                }
            }
        }
        WidgetDef::Bar { max, color, .. } => {
            if max.is_nan() || *max <= 0.0 {
                refs.error(format!("bar `{id}`: `max` is above 0"));
            }
            if let Some(color) = color.as_deref().filter(|c| c.starts_with('#'))
                && let Err(e) = parse_color(color)
            {
                refs.error(format!("bar `{id}`: {e}"));
            }
        }
        WidgetDef::Slider { min, max, step, value, setting, .. } => {
            if min.is_nan() || max.is_nan() || min >= max {
                refs.error(format!("slider `{id}`: `min` ({min}) is below `max` ({max})"));
            }
            if step.is_nan() || *step < 0.0 {
                refs.error(format!("slider `{id}`: `step` is 0 (any value) or more"));
            }
            if let Some(v) = value
                && !(min..=max).contains(&v)
            {
                refs.error(format!("slider `{id}`: `value` {v} is outside {min}..={max}"));
            }
            check_setting(&id, setting.as_deref(), refs);
        }
        WidgetDef::Toggle { setting, .. } => check_setting(&id, setting.as_deref(), refs),
        WidgetDef::Choice { options, labels, value, setting, .. } => {
            if options.is_empty() {
                refs.error(format!("choice `{id}`: no `options`"));
            }
            if !labels.is_empty() && labels.len() != options.len() {
                refs.error(format!(
                    "choice `{id}`: {} `labels` for {} `options`: one for each option, in their order (or none)",
                    labels.len(),
                    options.len()
                ));
            }
            if let Some(v) = value
                && !options.contains(v)
            {
                refs.error(format!("choice `{id}`: `value` `{v}` is not one of the options ({})", options.join(", ")));
            }
            check_setting(&id, setting.as_deref(), refs);
        }
        WidgetDef::Binding { context, action, taken, .. } => {
            // The player's controls are saved as `context.action`, split at the first dot.
            if context.is_empty() || context.contains('.') || context.contains(char::is_whitespace) {
                refs.error(format!("binding `{id}`: `context` is an input context's name (`gameplay`), without dots or spaces, not `{context}`"));
            }
            if action.is_empty() || action.contains(char::is_whitespace) {
                refs.error(format!("binding `{id}`: `action` is an action of the input map (`jump`, `light`), not `{action}`"));
            }
            if let Some(text) = taken
                && shards_locale::message_ref(text).is_none()
            {
                refs.error(format!(
                    "binding `{id}`: `taken` names a message with `$input` and `$action` (`@controls-taken`), not `{text}`"
                ));
            }
        }
        _ => {}
    }
    for child in widget.children() {
        check_widget(child, refs, seen, counters);
    }
}

fn check_setting(id: &str, setting: Option<&str>, refs: &mut Refs) {
    if let Some(key) = setting
        && key.split_once('.').is_none_or(|(group, field)| group.is_empty() || field.is_empty())
    {
        refs.error(format!("`{id}`: `setting` is `group.key` as the `settings` tool lists it (`accessibility.text_scale`), not `{key}`"));
    }
}

/// The id of `widget` in its screen: its own, or its kind and number in the screen (`text2`).
pub(crate) fn assign_id(widget: &WidgetDef, counters: &mut BTreeMap<&'static str, u32>) -> String {
    let n = counters.entry(widget.kind()).or_insert(0);
    *n += 1;
    match widget.id() {
        Some(id) => id.to_owned(),
        None => format!("{}{n}", widget.kind()),
    }
}

/// How screens look (kind `theme`, id `default`; the engine's own without one). Colours are
/// `#rrggbb` or `#rrggbbaa`, sizes logical pixels; the player's text size
/// (`accessibility.text_scale`) multiplies every font size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeDef {
    pub text: String,
    /// Disabled widgets and hints.
    pub dim_text: String,
    /// Behind a screen's widgets.
    pub panel: String,
    /// Over the game behind a menu.
    pub backdrop: String,
    pub button: String,
    pub button_hover: String,
    /// Bars without a colour, a toggle that is on, the active tab.
    pub accent: String,
    /// The outline of the focused widget.
    pub focus: String,
    /// Named colours for bars (`color: "health"`).
    pub colors: BTreeMap<String, String>,
    /// A font file under the asset folder (`fonts/FiraSans-Regular.ttf`) for every text. Default:
    /// the engine's own (`DEFAULT_FONT`: Fira Sans, with Latin, Greek and Cyrillic letters); a game
    /// in other scripts names one.
    pub font: Option<String>,
    pub font_size: f32,
    pub title_size: f32,
    pub hint_size: f32,
    pub spacing: f32,
    pub padding: f32,
    pub radius: f32,
    pub bar_width: f32,
    pub bar_height: f32,
    pub button_width: f32,
}

impl Default for ThemeDef {
    fn default() -> Self {
        let colors = [("health", "#c8423b"), ("stamina", "#4fae55"), ("poise", "#d9a441"), ("mana", "#4f7fd9")];
        Self {
            text: "#ece8e1".into(),
            dim_text: "#9c978f".into(),
            panel: "#15151cd9".into(),
            backdrop: "#00000080".into(),
            button: "#2a2a36".into(),
            button_hover: "#3a3a4a".into(),
            accent: "#d9a441".into(),
            focus: "#f5d76e".into(),
            colors: colors.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
            font: None,
            font_size: 18.0,
            title_size: 26.0,
            hint_size: 15.0,
            spacing: 8.0,
            padding: 12.0,
            radius: 6.0,
            bar_width: 240.0,
            bar_height: 10.0,
            button_width: 280.0,
        }
    }
}

impl ContentDef for ThemeDef {
    const KIND: &'static str = "theme";

    fn validate(&self, _id: &str, refs: &mut Refs) {
        let colors = [
            ("text", &self.text),
            ("dim_text", &self.dim_text),
            ("panel", &self.panel),
            ("backdrop", &self.backdrop),
            ("button", &self.button),
            ("button_hover", &self.button_hover),
            ("accent", &self.accent),
            ("focus", &self.focus),
        ];
        for (field, value) in colors.into_iter().chain(self.colors.iter().map(|(k, v)| (k.as_str(), v))) {
            if let Err(e) = parse_color(value) {
                refs.error(format!("{field}: {e}"));
            }
        }
        let sizes = [
            ("font_size", self.font_size),
            ("title_size", self.title_size),
            ("hint_size", self.hint_size),
            ("bar_width", self.bar_width),
            ("bar_height", self.bar_height),
            ("button_width", self.button_width),
        ];
        for (field, value) in sizes {
            if !(value > 0.0 && value.is_finite()) {
                refs.error(format!("{field}: a size above 0, not {value}"));
            }
        }
        for (field, value) in [("spacing", self.spacing), ("padding", self.padding), ("radius", self.radius)] {
            if !(value >= 0.0 && value.is_finite()) {
                refs.error(format!("{field}: 0 or more, not {value}"));
            }
        }
        if let Some(font) = &self.font
            && let Some(root) = refs.asset_root()
            && !root.join(font).is_file()
        {
            refs.error(format!("font: no file `{font}` under the asset folder (a .ttf or .otf, `fonts/FiraSans-Regular.ttf`)"));
        }
    }
}

impl ThemeDef {
    /// A bar's colour: its own `#rrggbb`, a name in [`colors`](Self::colors), or the accent.
    pub fn bar_color(&self, color: Option<&str>) -> [u8; 4] {
        let named = color.and_then(|c| if c.starts_with('#') { Some(c) } else { self.colors.get(c).map(String::as_str) });
        named.and_then(|c| parse_color(c).ok()).unwrap_or_else(|| self.color(&self.accent))
    }

    /// A colour of the theme; white where it does not read (content checks catch that).
    pub fn color(&self, text: &str) -> [u8; 4] {
        parse_color(text).unwrap_or([255; 4])
    }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` as sRGB bytes: content's colours, read as every kind reads them.
pub use shards_content::parse_color;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_read_and_mistakes_say_what_is_wrong() {
        assert_eq!(ButtonAction::parse("close"), Ok(ButtonAction::Close));
        assert_eq!(ButtonAction::parse(" open settings "), Ok(ButtonAction::Open("settings".into())));
        assert_eq!(ButtonAction::parse("quit"), Ok(ButtonAction::Quit));
        assert_eq!(ButtonAction::parse("respawn"), Ok(ButtonAction::Game("respawn".into())));
        assert_eq!(
            ButtonAction::parse(r#"tool use_item {"item": "flask"}"#),
            Ok(ButtonAction::Tool("use_item".into(), serde_json::json!({"item": "flask"})))
        );
        assert_eq!(ButtonAction::parse("tool stop"), Ok(ButtonAction::Tool("stop".into(), serde_json::json!({}))));
        assert!(ButtonAction::parse("open").unwrap_err().contains("takes one screen"));
        assert!(ButtonAction::parse("open a b").unwrap_err().contains("takes one screen"));
        assert!(ButtonAction::parse("close now").unwrap_err().contains("takes nothing after it"));
        assert!(ButtonAction::parse("tool x {oops").unwrap_err().contains("not JSON"));
        assert!(ButtonAction::parse("give gold").unwrap_err().contains("one word"));
        assert!(ButtonAction::parse("").unwrap_err().contains("empty action"));
    }

    #[test]
    fn colours_read_in_three_lengths() {
        assert_eq!(parse_color("#fff"), Ok([255, 255, 255, 255]));
        assert_eq!(parse_color("#c8423b"), Ok([200, 66, 59, 255]));
        assert_eq!(parse_color("#00000080"), Ok([0, 0, 0, 128]));
        assert!(parse_color("red").is_err());
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#gg0000").is_err());
        let theme = ThemeDef::default();
        assert_eq!(theme.bar_color(Some("health")), [200, 66, 59, 255]);
        assert_eq!(theme.bar_color(Some("#000000")), [0, 0, 0, 255]);
        assert_eq!(theme.bar_color(None), theme.color(&theme.accent));
        assert_eq!(theme.bar_color(Some("nope")), theme.color(&theme.accent), "an unknown name is the accent");
    }

    #[test]
    fn widgets_without_an_id_are_named_by_kind_and_number() {
        let mut counters = BTreeMap::new();
        let text = WidgetDef::Text { id: None, text: "a".into(), bind: None };
        let named = WidgetDef::Text { id: Some("title".into()), text: "b".into(), bind: None };
        assert_eq!(assign_id(&text, &mut counters), "text1");
        assert_eq!(assign_id(&named, &mut counters), "title");
        assert_eq!(assign_id(&text, &mut counters), "text3", "named ones count too, so adding an id renames no other");
    }

    #[test]
    fn a_screen_reads_from_ron() {
        let text = r#"(
            pauses: true,
            title: "Paused",
            widgets: [
                Button(id: "resume", label: "Resume", action: "close"),
                Slider(id: "size", label: "Text size", min: 0.5, max: 3.0, step: 0.25, setting: "accessibility.text_scale"),
                Tabs(id: "tabs", children: [Panel(id: "video", title: "Video", children: [Text(text: "hi")])]),
                Bar(label: "HP", bind: "you.health", color: "health"),
            ],
        )"#;
        let opts = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let screen: ScreenDef = opts.from_str(text).unwrap();
        assert!(screen.pauses && screen.closable && screen.layer == Layer::Menu);
        assert_eq!(screen.widgets.len(), 4);
        assert!(matches!(&screen.widgets[1], WidgetDef::Slider { step, setting: Some(s), .. } if *step == 0.25 && s == "accessibility.text_scale"));
        assert!(matches!(&screen.widgets[3], WidgetDef::Bar { max, .. } if *max == 1.0));
    }

    #[test]
    fn a_binding_reads_from_ron_with_its_defaults_and_writes_only_what_it_has() {
        let text = r#"[
            Binding(id: "jump", label: "@controls-jump", action: "jump"),
            Binding(id: "pad_back", label: "Back", context: "menu", action: "back", slot: 1, prompt: "@controls-press", taken: "@controls-taken"),
        ]"#;
        let opts = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let widgets: Vec<WidgetDef> = opts.from_str(text).unwrap();
        let WidgetDef::Binding { context, action, slot, prompt, taken, .. } = &widgets[0] else { panic!("{:?}", widgets[0]) };
        assert_eq!((context.as_str(), action.as_str(), slot, prompt, taken), ("gameplay", "jump", &None, &None, &None), "gameplay, every slot, the engine's texts");
        assert!(matches!(&widgets[1], WidgetDef::Binding { context, slot: Some(1), .. } if context == "menu"));
        assert_eq!((widgets[0].kind(), widgets[0].id()), ("binding", Some("jump")));
        assert_eq!(widgets[1].texts(), [("Back", "label"), ("@controls-press", "prompt"), ("@controls-taken", "taken")]);
        // What is left out stays out of the JSON the content database hashes.
        let json = serde_json::to_value(&widgets[0]).unwrap();
        assert_eq!(json, serde_json::json!({"Binding": {"id": "jump", "label": "@controls-jump", "action": "jump"}}));
        let menu = serde_json::to_value(&widgets[1]).unwrap();
        assert_eq!(menu["Binding"]["context"], "menu");
    }
}
