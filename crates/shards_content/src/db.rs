//! The content database: typed definitions by kind and id, each with the file it came
//! from, built from text and validated as a whole.
//!
//! One path builds it, whatever the text came from: the asset server at start, the
//! file watcher, `content_reload` and `content_check` reading the disk, or
//! [`check_dir`] in a game's CI. A replay changes it only through recorded
//! `content_apply` calls.
//!
//! It also notes what the simulation reads: a definition read by id, a kind read whole (its
//! ids, or every definition of it). Reads count while the world's [`SimScope`] is open (a fixed
//! tick, an action call, the world being built), and only for kinds the simulation depends on
//! ([`ContentDef::SIMULATION`]). The replay check `content` hashes those definitions
//! ([`ContentDb::used_parts`]), not every one: content a session never read can change without
//! its replays diverging. The co-op handshake compares the whole [hash](ContentDb::hash).

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ai_core::SimScope;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use ron::value::RawValue;
use schemars::JsonSchema;
use serde::de::{DeserializeOwned, DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Folder, inside the asset folder, that holds every content file.
pub const CONTENT_DIR: &str = "content";

/// A kind of content definition: `AttackDef` (kind `attack`) lives in
/// `<assets>/content/**/*.attack.ron`, each file a map of stable ids to definitions:
///
/// ```ron
/// {
///     "sword": (windup: 0.15, damage: 25.0),
///     "claw": (windup: 0.45, damage: 8.0),
/// }
/// ```
///
/// Ids are unique per kind across all files. `Option` fields may be written without
/// `Some(...)`. Add `#[serde(deny_unknown_fields)]`, or a typo in an optional field
/// (`damge: 8.0`) silently keeps its default. A definition that does not fit the type is an
/// error of its own, with the file, line and column; the other definitions of its file still
/// load. (A syntax error, a bad or repeated id breaks the map itself: that file adds nothing.)
/// Register a kind with [`ContentAppExt::add_content`](crate::ContentAppExt::add_content).
pub trait ContentDef: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// File kind: `"attack"` for `*.attack.ron`. Lowercase, no dots.
    const KIND: &'static str;

    /// Whether the simulation's outcome can depend on this kind. `false` for a kind that only
    /// shapes what a machine shows or plays (a look, an effect, a sound, a screen's theme) or
    /// what lies outside a session (settings, saves, Steam): reading it never counts for the
    /// replay check `content` (see the [module](self)), so editing it never makes a replay
    /// diverge. A kind the simulation reads keeps `true` (the default): a replay recorded with
    /// other numbers would otherwise play on without noticing.
    const SIMULATION: bool = true;

    /// Checks one definition against the whole database: report problems with
    /// [`Refs::error`], declare references with [`Refs::require`] (a missing one
    /// becomes an error naming the file, the id and the field).
    fn validate(&self, id: &str, refs: &mut Refs) {
        let _ = (id, refs);
    }
}

/// A problem with content, saying where: `content/combat/sword.moveset.ron:4:12:
/// moveset `hero`: light -> unknown action `sword_lite_1` (did you mean `sword_light_1`?)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ContentError {
    /// Asset path of the file: `content/combat/sword.moveset.ron`.
    pub file: Option<String>,
    /// `(line, column)` of a parse error in the file, 1-based.
    pub position: Option<(usize, usize)>,
    /// The content kind of the definition it concerns.
    pub kind: Option<String>,
    /// The definition it concerns.
    pub id: Option<String>,
    /// What is wrong, and how to fix it when that is not obvious.
    pub message: String,
}

impl ContentError {
    /// In `file`; none for a definition from no file (one `content_apply` added without `files`).
    fn in_file(file: &str, message: impl Into<String>) -> Self {
        let file = (!file.is_empty()).then(|| file.to_owned());
        Self { file, position: None, kind: None, id: None, message: message.into() }
    }
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            f.write_str(file)?;
            if let Some((line, col)) = self.position {
                write!(f, ":{line}:{col}")?;
            }
            f.write_str(": ")?;
        }
        match (&self.kind, &self.id) {
            (Some(kind), Some(id)) => write!(f, "{kind} `{id}`: ")?,
            (Some(kind), None) => write!(f, "{kind}: ")?,
            _ => {}
        }
        f.write_str(&self.message)
    }
}

/// What a clean check found: `ok: 14 definitions in 5 files`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Content files read.
    pub files: usize,
    /// Definitions in them.
    pub definitions: usize,
    /// Definitions per kind, every registered kind included.
    pub kinds: BTreeMap<String, usize>,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let defs = if self.definitions == 1 { "definition" } else { "definitions" };
        let files = if self.files == 1 { "file" } else { "files" };
        write!(f, "ok: {} {defs} in {} {files}", self.definitions, self.files)
    }
}

// --- kinds ---------------------------------------------------------------------------

pub(crate) type AnyDef = Arc<dyn Any + Send + Sync>;

/// Where a parse failed: position, the id being read, the message.
pub(crate) struct ParseFailure {
    position: Option<(usize, usize)>,
    id: Option<String>,
    message: String,
}

/// The definitions of one file in file order, each read or failed on its own; or the failure of
/// the file as a whole, when its map is broken (syntax, a bad or repeated id).
type Parsed = Result<Vec<(String, Result<AnyDef, ParseFailure>)>, ParseFailure>;

/// Monomorphized operations of one kind, so the database stays untyped.
#[derive(Clone, Copy)]
struct KindFns {
    parse: fn(&str) -> Parsed,
    from_json: fn(&Value) -> Result<AnyDef, String>,
    to_json: fn(&AnyDef) -> Result<Value, String>,
    validate: fn(&AnyDef, &str, &mut Refs),
}

/// A check of untyped definitions.
type CheckFn = Arc<dyn Fn(&AnyDef, &str, &mut Refs) + Send + Sync>;

/// A check another module adds to every definition of a kind ([`ContentKinds::add_check`]).
#[derive(Clone)]
struct KindCheck {
    kind: &'static str,
    name: &'static str,
    run: CheckFn,
}

/// `check` of kind `T`'s definitions, run on untyped ones.
fn erase_check<T: ContentDef>(check: fn(&T, &str, &mut Refs)) -> CheckFn {
    // The bound gives the closure the signature for every lifetime of its arguments.
    fn run<F: Fn(&AnyDef, &str, &mut Refs) + Send + Sync + 'static>(f: F) -> CheckFn {
        Arc::new(f)
    }
    run(move |def, id, refs| {
        if let Some(def) = def.downcast_ref::<T>() {
            check(def, id, refs);
        }
    })
}

/// One registered kind.
#[derive(Clone)]
pub struct KindInfo {
    /// `attack` for `*.attack.ron`.
    pub kind: &'static str,
    /// Rust type of its definitions.
    pub type_name: &'static str,
    /// [`ContentDef::SIMULATION`].
    pub simulation: bool,
    type_id: TypeId,
    fns: KindFns,
}

/// The registered content kinds, the ids the game requires, and the type registry
/// prefabs validate against. Built by [`add_content`](crate::ContentAppExt::add_content)
/// and [`require_content`](crate::ContentAppExt::require_content); [`check_dir`] needs it
/// to read files without an app running.
#[derive(Resource, Clone, Default)]
pub struct ContentKinds {
    kinds: Vec<KindInfo>,
    /// (kind, id, who needs it).
    required: Vec<(&'static str, String, String)>,
    /// Checks other modules add to kinds, in the order they were added.
    checks: Vec<KindCheck>,
    registry: Option<AppTypeRegistry>,
    /// The asset folder, for definitions that name asset files (levels).
    asset_root: Option<PathBuf>,
}

impl ContentKinds {
    /// Registers kind `T`. Registering it again changes nothing; another type under the
    /// same kind name is a bug, and panics.
    pub fn register<T: ContentDef>(&mut self) {
        assert!(
            !T::KIND.is_empty() && !T::KIND.contains(['.', '/', ' ']),
            "content kind `{}` must be a plain name (it is the file extension)",
            T::KIND
        );
        if let Some(existing) = self.get(T::KIND) {
            assert!(
                existing.type_id == TypeId::of::<T>(),
                "content kind `{}` is registered twice: {} and {}",
                T::KIND,
                existing.type_name,
                std::any::type_name::<T>()
            );
            return;
        }
        let fns = KindFns { parse: parse_file::<T>, from_json: def_from_json::<T>, to_json: def_to_json::<T>, validate: validate_def::<T> };
        let type_name = std::any::type_name::<T>();
        self.kinds.push(KindInfo { kind: T::KIND, type_name, simulation: T::SIMULATION, type_id: TypeId::of::<T>(), fns });
        self.kinds.sort_by_key(|k| k.kind);
    }

    /// Requires definition `id` of kind `T`: content without it has an error saying
    /// that `by` (the code that uses the id, e.g. `AttackKind`) needs it.
    pub fn require<T: ContentDef>(&mut self, id: &str, by: &str) {
        if !self.required.iter().any(|(kind, known, _)| *kind == T::KIND && known == id) {
            self.required.push((T::KIND, id.to_owned(), by.to_owned()));
        }
    }

    /// Runs `check` on every definition of kind `T` after its own
    /// [`validate`](ContentDef::validate), wherever content is validated (loading, a hot edit,
    /// `content check`): for what a module that does not own the kind knows of it (the umbrella
    /// crate reads a tile's level file, which neither the world nor the level module may do
    /// alone). Adding a check of the same `name` to the kind again changes nothing.
    pub fn add_check<T: ContentDef>(&mut self, name: &'static str, check: fn(&T, &str, &mut Refs)) {
        if self.checks.iter().any(|c| c.kind == T::KIND && c.name == name) {
            return;
        }
        self.checks.push(KindCheck { kind: T::KIND, name, run: erase_check(check) });
    }

    /// `json` as kind `kind` writes it, so `25` and `25.0` compare equal; `None` when it
    /// is not a definition of that kind.
    pub(crate) fn normalize(&self, kind: &str, json: &Value) -> Option<Value> {
        let info = self.get(kind)?;
        (info.fns.from_json)(json).ok().and_then(|def| (info.fns.to_json)(&def).ok())
    }

    /// The type registry prefabs (and other kinds that need reflection) validate with.
    pub fn set_type_registry(&mut self, registry: AppTypeRegistry) {
        self.registry = Some(registry);
    }

    /// The asset folder definitions that name asset files are checked against
    /// ([`Refs::asset_root`]). The content plugin sets it when loading starts and
    /// [`check_dir`] for its `base`.
    pub fn set_asset_root(&mut self, root: PathBuf) {
        self.asset_root = Some(root);
    }

    /// The registered kind `kind`.
    pub fn get(&self, kind: &str) -> Option<&KindInfo> {
        self.kinds.iter().find(|k| k.kind == kind)
    }

    /// Kind names, sorted.
    pub fn names(&self) -> Vec<&'static str> {
        self.kinds.iter().map(|k| k.kind).collect()
    }

    /// File extensions the asset loader claims: `attack.ron`, `prefab.ron`, ...
    pub fn extensions(&self) -> Vec<String> {
        self.kinds.iter().map(|k| format!("{}.ron", k.kind)).collect()
    }

    /// The kind of `content/combat/attacks.attack.ron`, or why there is none.
    fn of_file(&self, path: &str) -> Result<&KindInfo, String> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let stem = name.strip_suffix(".ron").ok_or_else(|| "not a .ron file".to_owned())?;
        let known = self.names().join(", ");
        let Some((_, kind)) = stem.rsplit_once('.') else {
            return Err(format!("content files are named `<name>.<kind>.ron` (kinds: {known})"));
        };
        self.get(kind).ok_or_else(|| {
            let hint = closest(kind, self.names().into_iter()).map(|k| format!(" (did you mean `{k}`?)")).unwrap_or_default();
            format!("no content kind `{kind}`{hint}; registered: {known}")
        })
    }
}

/// RON as content is written: `Some(...)` may be left out of `Option` fields.
pub(crate) fn ron_options() -> ron::Options {
    ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
}

/// Reads `{ "id": (definition), ... }` in file order, remembering which id was being
/// read when something failed. A definition stays the text it is written in ([`RawValue`]:
/// only its syntax is checked here), so one that does not fit its type fails alone when
/// [`parse_file`] reads it, and the file's other definitions still load.
struct FileSeed<'a> {
    current: &'a RefCell<Option<String>>,
}

impl<'de> DeserializeSeed<'de> for FileSeed<'_> {
    type Value = Vec<(String, &'de RawValue)>;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for FileSeed<'_> {
    type Value = Vec<(String, &'de RawValue)>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a map of definitions by id: { \"id\": (...), ... }")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut defs: Vec<(String, &'de RawValue)> = Vec::new();
        while let Some(id) = map.next_key::<String>()? {
            *self.current.borrow_mut() = Some(id.clone());
            if let Err(why) = check_id(&id) {
                return Err(serde::de::Error::custom(why));
            }
            if defs.iter().any(|(seen, _)| *seen == id) {
                return Err(serde::de::Error::custom("defined twice in this file"));
            }
            let def = map.next_value::<&'de RawValue>()?;
            defs.push((id, def));
            *self.current.borrow_mut() = None;
        }
        Ok(defs)
    }
}

/// Ids are names agents and files repeat: no blanks, nothing invisible.
fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("an id cannot be empty".into());
    }
    if id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("ids cannot contain spaces or control characters".into());
    }
    Ok(())
}

/// Reads a file in two steps. The map first, the way [`FileSeed`] cuts it into definitions
/// (a failure there is the file's: nothing says where the next definition starts). Then each
/// definition as `T`, alone, with the extensions the file turned on (`#![enable(...)]`).
fn parse_file<T: ContentDef>(text: &str) -> Parsed {
    let whole = |e: ron::error::SpannedError, id: Option<String>| ParseFailure {
        position: Some((e.span.start.line, e.span.start.col)),
        id,
        message: e.code.to_string(),
    };
    let current = RefCell::new(None);
    let options = ron_options();
    let mut de = ron::Deserializer::from_str_with_options(text, &options).map_err(|e| whole(e, None))?;
    let raws = match (FileSeed { current: &current }).deserialize(&mut de) {
        Ok(raws) => raws,
        Err(e) => return Err(whole(de.span_error(e), current.into_inner())),
    };
    de.end().map_err(|e| whole(de.span_error(e), None))?;
    let options = ron_options().with_default_extension(de.extensions());
    Ok(raws
        .into_iter()
        .map(|(id, raw)| {
            let ron = raw.get_ron();
            let def = options.from_str::<T>(ron).map(|def| Arc::new(def) as AnyDef).map_err(|e| ParseFailure {
                position: Some(position_in_file(text, ron, (e.span.start.line, e.span.start.col))),
                id: Some(id.clone()),
                message: e.code.to_string(),
            });
            (id, def)
        })
        .collect())
}

/// Where `at`, a `(line, column)` inside `part` (1-based, columns counted in chars as RON
/// does), is in `text`, when `part` is a piece of it: the line and column of `part`'s first
/// char, then `at` on from there. On `part`'s first line the column adds up; on later lines
/// it is the column of the file already. A `part` from elsewhere keeps `at` as it is.
fn position_in_file(text: &str, part: &str, at: (usize, usize)) -> (usize, usize) {
    let bounds = text.as_bytes().as_ptr_range();
    let start = part.as_ptr();
    if !bounds.contains(&start) {
        return at;
    }
    let before = &text[..start as usize - bounds.start as usize];
    let line = 1 + before.chars().filter(|&c| c == '\n').count();
    let col = 1 + before.chars().rev().take_while(|&c| c != '\n').count();
    if at.0 == 1 { (line, col + at.1 - 1) } else { (line + at.0 - 1, at.1) }
}

fn def_from_json<T: ContentDef>(json: &Value) -> Result<AnyDef, String> {
    T::deserialize(json).map(|def| Arc::new(def) as AnyDef).map_err(|e| e.to_string())
}

/// Through text, so floats keep their short form (`0.1`, not `0.10000000149011612`).
/// Fails for what JSON cannot hold (a map with keys that are not strings).
fn def_to_json<T: ContentDef>(def: &AnyDef) -> Result<Value, String> {
    let def = def.downcast_ref::<T>().ok_or("not a definition of this kind")?;
    let text = serde_json::to_string(def).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn validate_def<T: ContentDef>(def: &AnyDef, id: &str, refs: &mut Refs) {
    if let Some(def) = def.downcast_ref::<T>() {
        def.validate(id, refs);
    }
}

// --- the database ------------------------------------------------------------------

#[derive(Clone)]
struct Entry {
    file: String,
    def: AnyDef,
    /// The definition as JSON: compared for changes, hashed, shown by `content_get`.
    json: Value,
    /// FNV-1a of its kind, id and JSON: its part of the replay check `content`.
    hash: u64,
    /// The simulation has read it.
    used: Used,
}

impl Entry {
    fn new(kind: &str, id: &str, file: String, def: AnyDef, json: Value) -> Entry {
        let hash = def_hash(kind, id, &json);
        Entry { file, def, json, hash, used: Used::default() }
    }
}

/// A flag the simulation sets the first time it reads something, from any thread of a tick:
/// once it is set a read only loads it, so systems reading the same definition in parallel
/// share the cache line. A copy of the database copies the flag's value.
#[derive(Default)]
struct Used(AtomicBool);

impl Used {
    #[inline]
    fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    #[inline]
    fn mark(&self) {
        if !self.get() {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    fn put(&self, on: bool) {
        self.0.store(on, Ordering::Relaxed);
    }
}

impl Clone for Used {
    fn clone(&self) -> Self {
        Used(AtomicBool::new(self.get()))
    }
}

/// What the database knows of a kind besides its definitions.
#[derive(Clone, Default)]
struct KindState {
    /// [`ContentDef::SIMULATION`].
    simulation: bool,
    /// The simulation read the kind whole (its ids, or every definition).
    whole: Used,
}

/// FNV-1a over a definition's kind, id and JSON, each length-terminated, as
/// [`ContentDb::hash`] feeds them.
fn def_hash(kind: &str, id: &str, json: &Value) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes.iter().chain(&(bytes.len() as u64).to_le_bytes()) {
            h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    };
    feed(kind.as_bytes());
    feed(id.as_bytes());
    feed(json.to_string().as_bytes());
    h
}

/// The suffix of the part of the replay check `content` that says a kind was read whole
/// (`tile (all)`): every definition of the kind is a part then, those never read by id too.
pub const WHOLE_KIND: &str = " (all)";

/// One content file as text: what the asset server and disk reads hand over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Asset path: `content/combat/attacks.attack.ron`.
    pub path: String,
    /// The file's contents.
    pub text: String,
}

/// Every content definition, by kind and id. Read it with [`get`](Self::get) or the
/// [`Content`](crate::Content) system param; gameplay never writes it. Before loading
/// finishes it is empty ([`is_loaded`](Self::is_loaded) is false), and the simulation
/// is held.
#[derive(Resource, Clone, Default)]
pub struct ContentDb {
    /// Kind → id → entry, every registered kind present.
    tables: BTreeMap<&'static str, BTreeMap<String, Entry>>,
    /// Asset path → text as applied: the base a changed file is checked against.
    files: BTreeMap<String, String>,
    /// Problems in the files as they are (parse errors, duplicates, unknown kinds).
    file_errors: Vec<ContentError>,
    /// Problems between definitions (references, validation).
    def_errors: Vec<ContentError>,
    loaded: bool,
    /// Per registered kind: whether the simulation depends on it, and whether it read it whole.
    states: BTreeMap<&'static str, KindState>,
    /// The world's scope, while the database is its resource ([`ContentDb::watch`]): reads count
    /// while it is open. `None`: nothing counts (a database built to compare or check files).
    scope: Option<SimScope>,
}

impl ContentDb {
    /// Builds and validates a database from file texts (any order: files are read in
    /// path order, so the first of two duplicate ids is the one kept).
    pub fn build(kinds: &ContentKinds, sources: &[Source]) -> ContentDb {
        let mut db = Self::build_unchecked(kinds, sources);
        db.revalidate(kinds);
        db
    }

    /// [`build`](Self::build) without validating the definitions against each other: what the
    /// files say, for comparing definitions.
    pub(crate) fn build_unchecked(kinds: &ContentKinds, sources: &[Source]) -> ContentDb {
        let mut db = ContentDb { loaded: true, ..default() };
        for kind in &kinds.kinds {
            db.tables.insert(kind.kind, BTreeMap::new());
            db.states.insert(kind.kind, KindState { simulation: kind.simulation, whole: Used::default() });
        }
        let mut sources: Vec<&Source> = sources.iter().collect();
        sources.sort_by(|a, b| a.path.cmp(&b.path));
        for source in sources {
            db.add_file(kinds, source);
        }
        db
    }

    fn add_file(&mut self, kinds: &ContentKinds, source: &Source) {
        let path = source.path.as_str();
        self.files.insert(path.to_owned(), source.text.clone());
        let kind = match kinds.of_file(path) {
            Ok(kind) => kind,
            Err(why) => return self.file_errors.push(ContentError::in_file(path, why)),
        };
        let parse_error = |failure: ParseFailure| ContentError {
            file: Some(path.to_owned()),
            position: failure.position,
            kind: failure.id.is_some().then(|| kind.kind.to_owned()),
            id: failure.id,
            message: failure.message,
        };
        let defs = match (kind.fns.parse)(&source.text) {
            Ok(defs) => defs,
            Err(failure) => return self.file_errors.push(parse_error(failure)),
        };
        let table = self.tables.entry(kind.kind).or_default();
        for (id, def) in defs {
            // A definition that does not read is an error of its own: the rest of the file loads.
            let def = match def {
                Ok(def) => def,
                Err(failure) => {
                    self.file_errors.push(parse_error(failure));
                    continue;
                }
            };
            if let Some(first) = table.get(&id) {
                let message = format!("already defined in {}", first.file);
                let error = ContentError { kind: Some(kind.kind.to_owned()), id: Some(id), ..ContentError::in_file(path, message) };
                self.file_errors.push(error);
                continue;
            }
            // Tools, replays and reloads see definitions as JSON: one that has none would
            // compare equal to anything.
            let json = match (kind.fns.to_json)(&def) {
                Ok(json) => json,
                Err(why) => {
                    let message = format!("cannot be written as JSON: {why} (maps need string keys)");
                    let error = ContentError { kind: Some(kind.kind.to_owned()), id: Some(id), ..ContentError::in_file(path, message) };
                    self.file_errors.push(error);
                    continue;
                }
            };
            let entry = Entry::new(kind.kind, &id, path.to_owned(), def, json);
            table.insert(id, entry);
        }
    }

    /// Runs every definition's [`ContentDef::validate`] against the database as it is.
    fn revalidate(&mut self, kinds: &ContentKinds) {
        // Validation is not the simulation: what a check reads (`refs.db()`) does not count.
        let _quiet = self.scope.as_ref().map(SimScope::suspend);
        let guard = kinds.registry.as_ref().map(|r| r.read());
        let registry = guard.as_deref();
        let mut errors = Vec::new();
        for (kind, table) in &self.tables {
            let Some(info) = kinds.get(kind) else { continue };
            for (id, entry) in table {
                let asset_root = kinds.asset_root.as_deref();
                let mut refs = Refs { db: self, kind, id, file: &entry.file, registry, asset_root, errors: &mut errors };
                (info.fns.validate)(&entry.def, id, &mut refs);
                for check in kinds.checks.iter().filter(|c| c.kind == *kind) {
                    (check.run)(&entry.def, id, &mut refs);
                }
            }
        }
        for (kind, id, by) in &kinds.required {
            if self.entry(kind, id).is_none() {
                errors.push(ContentError {
                    file: None,
                    position: None,
                    kind: Some((*kind).to_owned()),
                    id: Some(id.clone()),
                    message: format!("missing; {by} needs it (define it in a content/**/*.{kind}.ron file)"),
                });
            }
        }
        self.def_errors = errors;
    }

    /// The definition `id` of kind `T`. In the simulation's scope it counts as read.
    pub fn get<T: ContentDef>(&self, id: &str) -> Option<&T> {
        let entry = self.tables.get(T::KIND)?.get(id)?;
        if T::SIMULATION && !entry.used.get() && self.counting() {
            entry.used.mark();
        }
        entry.def.downcast_ref::<T>()
    }

    /// Ids of kind `T`, sorted. In the simulation's scope the kind counts as read whole: a
    /// definition added to it can change what the reader does.
    pub fn ids<T: ContentDef>(&self) -> impl Iterator<Item = &str> {
        if T::SIMULATION {
            self.read_whole(T::KIND);
        }
        self.peek_ids::<T>()
    }

    /// Definitions of kind `T` by id, sorted. In the simulation's scope the kind counts as read
    /// whole.
    pub fn iter<T: ContentDef>(&self) -> impl Iterator<Item = (&str, &T)> {
        if T::SIMULATION {
            self.read_whole(T::KIND);
        }
        self.peek_iter::<T>()
    }

    /// [`get`](Self::get) for a reader that is not the simulation even inside its scope (a screen
    /// being shown, a describer): it never counts as read. Whatever the simulation's outcome
    /// depends on is read with [`get`](Self::get).
    pub fn peek<T: ContentDef>(&self, id: &str) -> Option<&T> {
        self.tables.get(T::KIND)?.get(id)?.def.downcast_ref::<T>()
    }

    /// [`ids`](Self::ids) that never counts as read (see [`peek`](Self::peek)).
    pub fn peek_ids<T: ContentDef>(&self) -> impl Iterator<Item = &str> {
        self.tables.get(T::KIND).into_iter().flat_map(|t| t.keys().map(String::as_str))
    }

    /// [`iter`](Self::iter) that never counts as read (see [`peek`](Self::peek)).
    pub fn peek_iter<T: ContentDef>(&self) -> impl Iterator<Item = (&str, &T)> {
        self.tables
            .get(T::KIND)
            .into_iter()
            .flat_map(|t| t.iter().filter_map(|(id, e)| Some((id.as_str(), e.def.downcast_ref::<T>()?))))
    }

    /// Reads count now: the world's scope is open.
    #[inline]
    fn counting(&self) -> bool {
        self.scope.as_ref().is_some_and(SimScope::is_open)
    }

    /// `kind` read whole, if reads count now.
    fn read_whole(&self, kind: &str) {
        if let Some(state) = self.states.get(kind)
            && state.simulation
            && !state.whole.get()
            && self.counting()
        {
            state.whole.mark();
        }
    }

    /// Makes this database count what the simulation of the world with `scope` reads: done
    /// wherever a database becomes a world's resource ([`install`](crate::install)).
    pub fn watch(&mut self, scope: Option<SimScope>) {
        self.scope = scope;
    }

    /// What the simulation has read, as the parts of the replay check `content`, in name order:
    /// `kind/id` with the definition's hash for each definition read by id, and for a kind read
    /// whole `kind (all)` and then every definition of it. Kinds the simulation does not depend
    /// on ([`ContentDef::SIMULATION`]) are never listed.
    pub fn used_parts(&self) -> Vec<(String, u64)> {
        let mut parts = Vec::new();
        for (kind, table) in &self.tables {
            let Some(state) = self.states.get(kind).filter(|s| s.simulation) else { continue };
            let whole = state.whole.get();
            if whole {
                parts.push((format!("{kind}{WHOLE_KIND}"), 0));
            }
            for (id, entry) in table.iter().filter(|(_, e)| whole || e.used.get()) {
                parts.push((format!("{kind}/{id}"), entry.hash));
            }
        }
        parts
    }

    /// Makes what counts as read exactly `parts` (names as [`used_parts`](Self::used_parts)
    /// writes them; a name of nothing here is left out): a snapshot load puts back what the
    /// session it was taken in had read.
    pub fn set_used(&self, parts: &BTreeSet<&str>) {
        for (kind, table) in &self.tables {
            if let Some(state) = self.states.get(kind) {
                state.whole.put(parts.contains(format!("{kind}{WHOLE_KIND}").as_str()));
            }
            for (id, entry) in table {
                entry.used.put(parts.contains(format!("{kind}/{id}").as_str()));
            }
        }
    }

    /// Whether `kind` is registered (never counts as read).
    pub fn has_kind(&self, kind: &str) -> bool {
        self.tables.contains_key(kind)
    }

    /// [`get_json`](Self::get_json) that never counts as read: the database's own machinery (an
    /// edit checked against what is there, a replay putting back its starting content).
    pub(crate) fn json_of(&self, kind: &str, id: &str) -> Option<&Value> {
        self.tables.get(kind)?.get(id).map(|e| &e.json)
    }

    /// The file definition `id` of kind `T` came from: `content/combat/attacks.attack.ron`.
    pub fn file_of<T: ContentDef>(&self, id: &str) -> Option<&str> {
        self.file(T::KIND, id)
    }

    /// Registered kinds, sorted (only after loading).
    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tables.keys().copied()
    }

    /// Ids of `kind`, sorted; `None` for a kind nobody registered. In the simulation's scope the
    /// kind counts as read whole, as with [`ids`](Self::ids).
    pub fn ids_of(&self, kind: &str) -> Option<Vec<&str>> {
        let table = self.tables.get(kind)?;
        self.read_whole(kind);
        Some(table.keys().map(String::as_str).collect())
    }

    /// The definition as JSON, for tools and other untyped readers. In the simulation's scope it
    /// counts as read, as with [`get`](Self::get).
    pub fn get_json(&self, kind: &str, id: &str) -> Option<&Value> {
        let entry = self.tables.get(kind)?.get(id)?;
        if !entry.used.get() && self.states.get(kind).is_some_and(|s| s.simulation) && self.counting() {
            entry.used.mark();
        }
        Some(&entry.json)
    }

    /// The file a definition of `kind` came from.
    pub fn file(&self, kind: &str, id: &str) -> Option<&str> {
        self.tables.get(kind)?.get(id).map(|e| e.file.as_str())
    }

    /// Content files by asset path.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// The text of a content file as applied.
    pub fn file_text(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// Every problem found: files first, then references.
    pub fn errors(&self) -> impl Iterator<Item = &ContentError> {
        self.file_errors.iter().chain(&self.def_errors)
    }

    /// Loading finished (with or without errors).
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Number of definitions.
    pub fn len(&self) -> usize {
        self.tables.values().map(BTreeMap::len).sum()
    }

    /// No definitions at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Files and definitions, per kind.
    pub fn summary(&self) -> Summary {
        Summary {
            files: self.files.len(),
            definitions: self.len(),
            kinds: self.tables.iter().map(|(k, t)| ((*k).to_owned(), t.len())).collect(),
        }
    }

    /// FNV-1a over every definition (kind, id, JSON) in order: what replay checks
    /// compare, so a recording made with other numbers diverges at its first check.
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for b in bytes.iter().chain(&(bytes.len() as u64).to_le_bytes()) {
                h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
            }
        };
        for (kind, table) in &self.tables {
            for (id, entry) in table {
                feed(kind.as_bytes());
                feed(id.as_bytes());
                feed(entry.json.to_string().as_bytes());
            }
        }
        h
    }

    fn entry(&self, kind: &str, id: &str) -> Option<&Entry> {
        self.tables.get(kind)?.get(id)
    }

    /// The definition changes that turn `self` into `other`, in kind and id order.
    pub(crate) fn changes_to(&self, other: &ContentDb) -> Vec<DefChange> {
        let mut keys: Vec<(&str, &str)> = Vec::new();
        for db in [self, other] {
            for (kind, table) in &db.tables {
                keys.extend(table.keys().map(|id| (*kind, id.as_str())));
            }
        }
        keys.sort_unstable();
        keys.dedup();
        keys.into_iter()
            .filter_map(|(kind, id)| {
                let (old, new) = (self.entry(kind, id), other.entry(kind, id));
                let same = matches!((old, new), (Some(o), Some(n)) if o.json == n.json && o.file == n.file);
                (!same).then(|| DefChange {
                    kind: kind.to_owned(),
                    id: id.to_owned(),
                    file: new.or(old).map(|e| e.file.clone()),
                    def: new.map(|e| e.json.clone()),
                    before: old.map(|e| e.json.clone()),
                })
            })
            .collect()
    }

    /// A copy with `changes` applied (definitions read from their JSON) and references
    /// checked again. `files`: the files the changes came from, whose old file errors
    /// no longer apply.
    pub(crate) fn with_changes(&self, kinds: &ContentKinds, files: &[String], changes: &[DefChange]) -> Result<ContentDb, String> {
        let mut db = self.clone();
        for change in changes {
            let info = kinds.get(&change.kind).ok_or_else(|| {
                format!("no content kind `{}`; registered: {}", change.kind, kinds.names().join(", "))
            })?;
            db.states.entry(info.kind).or_insert_with(|| KindState { simulation: info.simulation, whole: Used::default() });
            let table = db.tables.entry(info.kind).or_default();
            match &change.def {
                Some(json) => {
                    let what = |e: String| format!("{} `{}`: {e}", change.kind, change.id);
                    let def = (info.fns.from_json)(json).map_err(what)?;
                    let file = change.file.clone().or_else(|| table.get(&change.id).map(|e| e.file.clone())).unwrap_or_default();
                    let json = (info.fns.to_json)(&def).map_err(what)?;
                    let entry = Entry::new(info.kind, &change.id, file, def, json);
                    // A definition the session read is still read: the replay check hashes its
                    // new version from now on.
                    if let Some(old) = table.get(&change.id) {
                        entry.used.put(old.used.get());
                    }
                    table.insert(change.id.clone(), entry);
                }
                None => {
                    table.remove(&change.id);
                }
            }
        }
        db.file_errors.retain(|e| e.file.as_ref().is_none_or(|f| !files.contains(f)));
        db.revalidate(kinds);
        Ok(db)
    }

    /// Errors in `self` that `before` did not have.
    pub(crate) fn new_errors<'a>(&'a self, before: &ContentDb) -> Vec<&'a ContentError> {
        let old: Vec<&ContentError> = before.errors().collect();
        self.errors().filter(|e| !old.contains(e)).collect()
    }

    /// What stops an edit of `edited` files from applying: errors `before` did not have, and
    /// any definition of an edited file that does not read, new error or not. A file applies
    /// whole or not at all, so one with a broken definition keeps its previous version until
    /// every definition reads. The file errors are the ones that leave a definition out: it
    /// does not parse (with a position), its id is taken, it has no JSON (without one).
    pub(crate) fn refusals<'a>(&'a self, before: &ContentDb, edited: &[String]) -> Vec<&'a ContentError> {
        let old: Vec<&ContentError> = before.errors().collect();
        let unread = |e: &ContentError| {
            self.file_errors.iter().any(|f| std::ptr::eq(f, e)) && e.file.as_ref().is_some_and(|f| edited.contains(f))
        };
        self.errors().filter(|e| !old.contains(e) || unread(e)).collect()
    }

    /// Takes the file texts and file errors of `other` (after its definitions were
    /// applied): what later edits are checked against.
    pub(crate) fn adopt_files(&mut self, other: &ContentDb) {
        self.files = other.files.clone();
        self.file_errors = other.file_errors.clone();
        self.def_errors = other.def_errors.clone();
    }

    /// File texts as applied, as sources to build a candidate from.
    pub(crate) fn sources(&self) -> BTreeMap<String, String> {
        self.files.clone()
    }
}

/// One definition added, replaced or removed: what `content_apply` takes and replays
/// record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DefChange {
    /// Content kind: `attack`.
    pub kind: String,
    /// The definition's id.
    pub id: String,
    /// The file that holds it (held it, when removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The new definition as JSON; absent when removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub def: Option<Value>,
    /// The definition before the change; absent when it is new. Playing a replay back
    /// starts from it, so the replay reproduces the session even after the file changed
    /// on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
}

// --- validation ----------------------------------------------------------------------

/// What [`ContentDef::validate`] sees: the whole candidate database, and where to
/// report problems.
pub struct Refs<'a> {
    db: &'a ContentDb,
    kind: &'static str,
    id: &'a str,
    file: &'a str,
    registry: Option<&'a TypeRegistry>,
    asset_root: Option<&'a Path>,
    errors: &'a mut Vec<ContentError>,
}

impl<'a> Refs<'a> {
    /// Declares that `field` refers to definition `id` of kind `T`. A missing one is an
    /// error that names the file, this definition and the field, with the closest
    /// existing id: `light -> unknown action `sword_lite_1` (did you mean
    /// `sword_light_1`?)`. Returns whether it exists.
    pub fn require<T: ContentDef>(&mut self, id: &str, field: &str) -> bool {
        if self.db.entry(T::KIND, id).is_some() {
            return true;
        }
        let mut message = format!("{field} -> unknown {} `{id}`", T::KIND);
        match self.db.ids_of(T::KIND) {
            Some(ids) => {
                if let Some(close) = closest(id, ids.into_iter()) {
                    message.push_str(&format!(" (did you mean `{close}`?)"));
                }
            }
            None => message.push_str(&format!(" (no content kind `{}` is registered)", T::KIND)),
        }
        self.error(message);
        false
    }

    /// Another definition, from the database being validated.
    pub fn get<T: ContentDef>(&self, id: &str) -> Option<&'a T> {
        self.db.get::<T>(id)
    }

    /// Every definition of kind `T` by id, sorted: for checks across definitions
    /// (unique wire ids).
    pub fn iter<T: ContentDef>(&self) -> impl Iterator<Item = (&'a str, &'a T)> + 'a {
        self.db.iter::<T>()
    }

    /// Reports a problem with this definition.
    pub fn error(&mut self, message: impl Into<String>) {
        self.errors.push(ContentError {
            kind: Some(self.kind.to_owned()),
            id: Some(self.id.to_owned()),
            ..ContentError::in_file(self.file, message)
        });
    }

    /// The app's type registry, for definitions that name components (prefabs).
    /// `None` when checking files without an app.
    pub fn type_registry(&self) -> Option<&'a TypeRegistry> {
        self.registry
    }

    /// The asset folder, for definitions that name asset files (a level's glTF) and
    /// check them. `None` when the database was built without one.
    pub fn asset_root(&self) -> Option<&'a Path> {
        self.asset_root
    }

    /// The whole database being validated, for checks that need more than [`get`](Self::get)
    /// (a level file's prefab nodes).
    pub fn db(&self) -> &'a ContentDb {
        self.db
    }
}

/// The candidate closest to `wanted` by edit distance, if it is close enough to be a
/// typo.
pub fn closest<'a>(wanted: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (wanted.chars().count() / 3).max(2);
    candidates
        .map(|c| (edit_distance(wanted, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)))
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

// --- files on disk -------------------------------------------------------------------

/// Where to look when there is no content folder.
pub(crate) const NO_FOLDER_HINT: &str = "games run from cargo find `assets` in the crate folder; elsewhere it is next \
     to the executable, or where BEVY_ASSET_ROOT points";

/// Asset paths (`content/...`, forward slashes) of every `.ron` file under
/// `<base>/content`, sorted. No `content` folder: no files.
pub(crate) fn list_files(base: &Path) -> Result<Vec<String>, ContentError> {
    let mut paths = Vec::new();
    let root = base.join(CONTENT_DIR);
    if root.is_dir() {
        walk(&root, CONTENT_DIR, &mut paths)?;
    }
    paths.sort();
    Ok(paths)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), ContentError> {
    let fail = |e: std::io::Error| ContentError::in_file(rel, format!("cannot read {}: {e}", dir.display()));
    for entry in std::fs::read_dir(dir).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{rel}/{name}");
        if entry.file_type().map_err(fail)?.is_dir() {
            walk(&entry.path(), &path, out)?;
        } else if name.ends_with(".ron") {
            out.push(path);
        }
    }
    Ok(())
}

/// The text of the content file at asset path `path` under `base`.
pub(crate) fn read_file(base: &Path, path: &str) -> Result<Source, ContentError> {
    let text = std::fs::read(base.join(path)).map_err(|e| ContentError::in_file(path, format!("cannot read: {e}")))?;
    Ok(Source { path: path.to_owned(), text: String::from_utf8_lossy(&text).into_owned() })
}

/// Every content file under `<base>/content` with its text, sorted by path.
pub fn read_dir(base: &Path) -> Result<Vec<Source>, ContentError> {
    list_files(base)?.iter().map(|path| read_file(base, path)).collect()
}

/// Whether some registered kind claims the file at `path`.
pub(crate) fn is_claimed(kinds: &ContentKinds, path: &str) -> bool {
    kinds.of_file(path).is_ok()
}

/// Reads, parses and validates every content file under `<base>/content` without an
/// app: for a game's `--check-content` in CI. `base` is the asset folder. A missing
/// `content` folder is an error: a check that looked in the wrong place must not pass.
pub fn check_dir(base: &Path, kinds: &ContentKinds) -> Result<Summary, Vec<ContentError>> {
    let folder = base.join(CONTENT_DIR);
    if !folder.is_dir() {
        let message = format!("no content folder at {} ({NO_FOLDER_HINT})", folder.display());
        return Err(vec![ContentError { file: None, position: None, kind: None, id: None, message }]);
    }
    let sources = read_dir(base).map_err(|e| vec![e])?;
    let mut kinds = kinds.clone();
    kinds.set_asset_root(base.to_owned());
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<ContentError> = db.errors().cloned().collect();
    if errors.is_empty() { Ok(db.summary()) } else { Err(errors) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typos_find_their_closest_id() {
        let ids = ["sword_light_1", "sword_light_2", "sword_heavy"];
        assert_eq!(closest("sword_lite_1", ids.into_iter()), Some("sword_light_1"));
        assert_eq!(closest("axe", ids.into_iter()), None);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn errors_read_like_a_compiler() {
        let e = ContentError {
            file: Some("content/combat/sword.moveset.ron".into()),
            position: Some((4, 12)),
            kind: Some("moveset".into()),
            id: Some("hero".into()),
            message: "light -> unknown action `sword_lite_1`".into(),
        };
        assert_eq!(e.to_string(), "content/combat/sword.moveset.ron:4:12: moveset `hero`: light -> unknown action `sword_lite_1`");
        assert_eq!(Summary { files: 1, definitions: 1, ..default() }.to_string(), "ok: 1 definition in 1 file");
    }

    #[test]
    fn a_definition_json_cannot_hold_is_an_error_not_null() {
        #[derive(Serialize, Deserialize, Clone)]
        struct Grid {
            cells: BTreeMap<(u8, u8), f32>,
        }
        impl ContentDef for Grid {
            const KIND: &'static str = "grid";
        }
        let mut kinds = ContentKinds::default();
        kinds.register::<Grid>();
        let source = Source { path: "content/a.grid.ron".into(), text: r#"{ "g": (cells: {(1, 2): 3.0}) }"#.into() };
        let db = ContentDb::build(&kinds, &[source]);
        let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
        assert_eq!(errors, ["content/a.grid.ron: grid `g`: cannot be written as JSON: key must be a string (maps need string keys)"]);
        assert!(db.get::<Grid>("g").is_none());
    }

    #[test]
    fn a_check_another_module_adds_runs_after_the_kinds_own_once_per_name() {
        #[derive(Serialize, Deserialize, Clone)]
        struct Door {
            width: f32,
        }
        impl ContentDef for Door {
            const KIND: &'static str = "door";
            fn validate(&self, _: &str, refs: &mut Refs) {
                if self.width <= 0.0 {
                    refs.error("width must be above 0");
                }
            }
        }
        fn fits(door: &Door, _: &str, refs: &mut Refs) {
            if door.width > 3.0 {
                refs.error(format!("width {} is wider than a wall's 3 m", door.width));
            }
        }
        let mut kinds = ContentKinds::default();
        kinds.register::<Door>();
        kinds.add_check::<Door>("walls", fits);
        kinds.add_check::<Door>("walls", fits);
        let source = Source { path: "content/a.door.ron".into(), text: r#"{ "gate": (width: 4.0), "hatch": (width: -1.0) }"#.into() };
        let errors: Vec<String> = ContentDb::build(&kinds, &[source]).errors().map(ToString::to_string).collect();
        assert_eq!(
            errors,
            ["content/a.door.ron: door `gate`: width 4 is wider than a wall's 3 m", "content/a.door.ron: door `hatch`: width must be above 0"]
        );
    }

    #[derive(Serialize, Deserialize, Clone, Debug)]
    #[serde(deny_unknown_fields)]
    struct Strike {
        damage: f32,
        #[serde(default)]
        windup: f32,
    }
    impl ContentDef for Strike {
        const KIND: &'static str = "strike";
    }

    /// The only error of a database built from `text` as one `strike` file: its position,
    /// the id it names and its message.
    fn only_error(text: &str) -> ((usize, usize), String, String) {
        let mut kinds = ContentKinds::default();
        kinds.register::<Strike>();
        let db = ContentDb::build(&kinds, &[Source { path: "content/a.strike.ron".into(), text: text.into() }]);
        let errors: Vec<&ContentError> = db.errors().collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        (errors[0].position.expect("a position"), errors[0].id.clone().expect("an id"), errors[0].message.clone())
    }

    #[test]
    fn a_broken_definition_is_reported_where_a_whole_file_parse_reported_it() {
        // Each text has one mistake, in a different place and in a different kind of way.
        let cases = [
            ("on the line of the id", "{ \"claw\": (damge: 8.0) }", "claw"),
            ("after good definitions on the same line", "{ \"a\": (damage: 1.0), \"b\": (damge: 2.0) }", "b"),
            ("on a later line of the definition", "{\n    \"sword\": (damage: 25.0),\n    \"claw\": (\n        damage: 8.0,\n        speed: 3.0,\n    ),\n}", "claw"),
            ("in a value that starts on the next line", "{\n    \"claw\":\n        (damge: 8.0),\n}", "claw"),
            ("behind a comment", "{\n    \"claw\": /* heavy */ (damge: 8.0), // slow\n}", "claw"),
            ("behind an id of wide chars", "{ \"клык\": (damge: 8.0) }", "клык"),
            ("with tabs and CRLF", "{\r\n\t\"a\": (damage: 1.0),\r\n\t\"b\": (\r\n\t\twindup: 0.5, damge: 8.0,\r\n\t),\r\n}\r\n", "b"),
            ("a wrong type", "{\n    \"claw\": (damage: \"lots\"),\n}", "claw"),
            ("a missing field", "{\n    \"sword\": (damage: 1.0),\n    \"claw\": (windup: 0.2),\n}", "claw"),
            ("a value of the wrong shape", "{ \"claw\": 12 }", "claw"),
        ];
        for (what, text, id) in cases {
            let old = ron_options().from_str::<BTreeMap<String, Strike>>(text).unwrap_err();
            let (position, named, message) = only_error(text);
            assert_eq!(position, (old.span.start.line, old.span.start.col), "position {what}");
            assert_eq!(named, id, "id {what}");
            assert_eq!(message, old.code.to_string(), "message {what}");
        }
    }

    #[test]
    fn a_position_inside_a_piece_of_a_file_is_a_position_in_the_file() {
        let text = "{\n  \"клык\": (a: 1,\n    b: 2),\n}";
        let part = &text[text.find('(').unwrap()..];
        // The piece starts at line 2, column 11 (the id's four chars are two bytes each).
        assert_eq!(position_in_file(text, part, (1, 1)), (2, 11));
        assert_eq!(position_in_file(text, part, (1, 5)), (2, 15));
        assert_eq!(position_in_file(text, part, (2, 7)), (3, 7));
        // Text that is not a piece of the file is left alone.
        assert_eq!(position_in_file(text, "(a: 1)", (1, 3)), (1, 3));
    }

    /// A kind only a picture reads.
    #[derive(Serialize, Deserialize, Clone, Debug)]
    struct Glow {
        strength: f32,
    }
    impl ContentDef for Glow {
        const KIND: &'static str = "glow";
        const SIMULATION: bool = false;
    }

    /// Strikes `a`, `b`, `c` and glows `soft`, `hard`, watched by a scope of their own.
    fn watched() -> (ContentDb, SimScope, ContentKinds) {
        let mut kinds = ContentKinds::default();
        kinds.register::<Strike>();
        kinds.register::<Glow>();
        let sources = [
            Source { path: "content/a.strike.ron".into(), text: r#"{ "a": (damage: 1.0), "b": (damage: 2.0), "c": (damage: 3.0) }"#.into() },
            Source { path: "content/a.glow.ron".into(), text: r#"{ "soft": (strength: 0.2), "hard": (strength: 0.9) }"#.into() },
        ];
        let mut db = ContentDb::build(&kinds, &sources);
        assert_eq!(db.errors().count(), 0);
        let scope = SimScope::default();
        db.watch(Some(scope.clone()));
        (db, scope, kinds)
    }

    fn names(db: &ContentDb) -> Vec<String> {
        db.used_parts().into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn only_the_simulations_reads_count_and_only_of_kinds_it_depends_on() {
        let (db, scope, _) = watched();
        // Closed: a frame, a query, a check.
        db.get::<Strike>("a");
        db.iter::<Strike>().count();
        assert!(names(&db).is_empty());
        // Open, but suspended: a rule or a hash inside a tick.
        let open = scope.open();
        let quiet = scope.suspend();
        db.get::<Strike>("a");
        db.get_json("strike", "c");
        assert!(names(&db).is_empty());
        drop(quiet);
        // Open: a tick, an action call.
        db.get::<Strike>("b");
        db.get_json("strike", "c");
        db.peek::<Strike>("a");
        db.peek_iter::<Strike>().count();
        db.get::<Glow>("soft");
        db.iter::<Glow>().count();
        db.get::<Strike>("nothing");
        drop(open);
        db.get::<Strike>("a");
        assert_eq!(names(&db), ["strike/b", "strike/c"], "peeks, a picture's kind and a missing id are not reads");
        // The part's value is the definition's own hash: its kind, id and JSON.
        let (_, hash) = db.used_parts()[0].clone();
        assert_eq!(hash, def_hash("strike", "b", db.json_of("strike", "b").unwrap()));
    }

    #[test]
    fn a_kind_read_whole_lists_every_definition_of_it() {
        for read in ["iter", "ids", "ids_of"] {
            let (db, scope, _) = watched();
            let _open = scope.open();
            db.get::<Strike>("a");
            match read {
                "iter" => assert_eq!(db.iter::<Strike>().count(), 3),
                "ids" => assert_eq!(db.ids::<Strike>().count(), 3),
                _ => assert_eq!(db.ids_of("strike").map(|ids| ids.len()), Some(3)),
            }
            assert_eq!(names(&db), ["strike (all)", "strike/a", "strike/b", "strike/c"], "{read}");
            // A picture's kind read whole is still not a part.
            db.ids_of("glow");
            assert_eq!(names(&db).len(), 4, "{read}");
        }
    }

    #[test]
    fn what_was_read_survives_an_edit_and_a_snapshot_puts_it_back_exactly() {
        let (db, scope, kinds) = watched();
        {
            let _open = scope.open();
            db.get::<Strike>("a");
            db.get::<Strike>("b");
        }
        let before = db.used_parts();
        // `content_apply` replaces `a` (read) and adds `d` (never read): `a` is read still, with
        // its new hash; `d` is not a part.
        let changes = [
            DefChange { kind: "strike".into(), id: "a".into(), file: None, def: Some(serde_json::json!({"damage": 5.0})), before: None },
            DefChange { kind: "strike".into(), id: "d".into(), file: None, def: Some(serde_json::json!({"damage": 9.0})), before: None },
            DefChange { kind: "strike".into(), id: "b".into(), file: None, def: None, before: None },
        ];
        let next = db.with_changes(&kinds, &[], &changes).unwrap();
        let after = next.used_parts();
        assert_eq!(after.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["strike/a"], "b went with its definition");
        assert_ne!(after[0].1, before[0].1, "a's part has the new version's hash");
        // A copy counts for the same world: reads in its scope mark the copy.
        {
            let _open = scope.open();
            next.get::<Strike>("d");
        }
        assert_eq!(names(&next), ["strike/a", "strike/d"]);
        assert_eq!(names(&db), ["strike/a", "strike/b"], "the original keeps its own flags");
        // A snapshot load makes the reads exactly the snapshot's.
        next.set_used(&["strike (all)", "strike/c", "glow/soft", "nothing/here"].into_iter().collect());
        assert_eq!(names(&next), ["strike (all)", "strike/a", "strike/c", "strike/d"]);
        next.set_used(&BTreeSet::new());
        assert!(names(&next).is_empty());
    }

    #[test]
    fn validation_reads_nothing_even_inside_the_scope() {
        #[derive(Serialize, Deserialize, Clone)]
        struct Combo {
            strikes: Vec<String>,
        }
        impl ContentDef for Combo {
            const KIND: &'static str = "combo";
            fn validate(&self, _: &str, refs: &mut Refs) {
                for strike in &self.strikes {
                    refs.require::<Strike>(strike, "strikes");
                    refs.get::<Strike>(strike);
                }
                refs.iter::<Strike>().count();
                refs.db().ids_of("strike");
            }
        }
        let (db, scope, mut kinds) = watched();
        kinds.register::<Combo>();
        let change = DefChange { kind: "combo".into(), id: "x".into(), file: None, def: Some(serde_json::json!({"strikes": ["a", "b"]})), before: None };
        let _open = scope.open();
        let next = db.with_changes(&kinds, &[], &[change]).unwrap();
        assert_eq!(next.errors().count(), 0);
        assert!(names(&next).is_empty(), "{:?}", names(&next));
        assert!(scope.is_open(), "the suspension ended with the validation");
    }
}
