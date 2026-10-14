//! The content database: typed definitions by kind and id, each with the file it came
//! from, built from text and validated as a whole.
//!
//! One path builds it, whatever the text came from: the asset server at start, the
//! file watcher, `content_reload` and `content_check` reading the disk, or
//! [`check_dir`] in a game's CI. A replay changes it only through recorded
//! `content_apply` calls, so its [hash](ContentDb::hash) is part of every replay check.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
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
/// (`damge: 8.0`) silently keeps its default. Register a kind with
/// [`ContentAppExt::add_content`](crate::ContentAppExt::add_content).
pub trait ContentDef: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// File kind: `"attack"` for `*.attack.ron`. Lowercase, no dots.
    const KIND: &'static str;

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
    /// `(line, column)` of a parse error, 1-based.
    pub position: Option<(usize, usize)>,
    /// The content kind of the definition it concerns.
    pub kind: Option<String>,
    /// The definition it concerns.
    pub id: Option<String>,
    /// What is wrong, and how to fix it when that is not obvious.
    pub message: String,
}

impl ContentError {
    fn in_file(file: &str, message: impl Into<String>) -> Self {
        Self { file: Some(file.to_owned()), position: None, kind: None, id: None, message: message.into() }
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

/// Definitions of one file in file order, or where it failed.
type Parsed = Result<Vec<(String, AnyDef)>, ParseFailure>;

/// Monomorphized operations of one kind, so the database stays untyped.
#[derive(Clone, Copy)]
struct KindFns {
    parse: fn(&str) -> Parsed,
    from_json: fn(&Value) -> Result<AnyDef, String>,
    to_json: fn(&AnyDef) -> Result<Value, String>,
    validate: fn(&AnyDef, &str, &mut Refs),
}

/// One registered kind.
#[derive(Clone)]
pub struct KindInfo {
    /// `attack` for `*.attack.ron`.
    pub kind: &'static str,
    /// Rust type of its definitions.
    pub type_name: &'static str,
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
    registry: Option<AppTypeRegistry>,
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
        self.kinds.push(KindInfo { kind: T::KIND, type_name: std::any::type_name::<T>(), type_id: TypeId::of::<T>(), fns });
        self.kinds.sort_by_key(|k| k.kind);
    }

    /// Requires definition `id` of kind `T`: content without it has an error saying
    /// that `by` (the code that uses the id, e.g. `AttackKind`) needs it.
    pub fn require<T: ContentDef>(&mut self, id: &str, by: &str) {
        if !self.required.iter().any(|(kind, known, _)| *kind == T::KIND && known == id) {
            self.required.push((T::KIND, id.to_owned(), by.to_owned()));
        }
    }

    /// The type registry prefabs (and other kinds that need reflection) validate with.
    pub fn set_type_registry(&mut self, registry: AppTypeRegistry) {
        self.registry = Some(registry);
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
/// read when something failed.
struct FileSeed<'a, T> {
    current: &'a std::cell::RefCell<Option<String>>,
    _def: std::marker::PhantomData<T>,
}

impl<'de, T: DeserializeOwned> DeserializeSeed<'de> for FileSeed<'_, T> {
    type Value = Vec<(String, T)>;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de, T: DeserializeOwned> Visitor<'de> for FileSeed<'_, T> {
    type Value = Vec<(String, T)>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a map of definitions by id: { \"id\": (...), ... }")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut defs: Vec<(String, T)> = Vec::new();
        while let Some(id) = map.next_key::<String>()? {
            *self.current.borrow_mut() = Some(id.clone());
            if let Err(why) = check_id(&id) {
                return Err(serde::de::Error::custom(why));
            }
            if defs.iter().any(|(seen, _)| *seen == id) {
                return Err(serde::de::Error::custom("defined twice in this file"));
            }
            let def = map.next_value::<T>()?;
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

fn parse_file<T: ContentDef>(text: &str) -> Parsed {
    let current = std::cell::RefCell::new(None);
    let seed = FileSeed::<T> { current: &current, _def: std::marker::PhantomData };
    match ron_options().from_str_seed(text, seed) {
        Ok(defs) => Ok(defs.into_iter().map(|(id, def)| (id, Arc::new(def) as AnyDef)).collect()),
        Err(e) => Err(ParseFailure {
            position: Some((e.span.start.line, e.span.start.col)),
            id: current.into_inner(),
            message: e.code.to_string(),
        }),
    }
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
}

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
}

impl ContentDb {
    /// Builds and validates a database from file texts (any order: files are read in
    /// path order, so the first of two duplicate ids is the one kept).
    pub fn build(kinds: &ContentKinds, sources: &[Source]) -> ContentDb {
        let mut db = ContentDb { loaded: true, ..default() };
        for kind in &kinds.kinds {
            db.tables.insert(kind.kind, BTreeMap::new());
        }
        let mut sources: Vec<&Source> = sources.iter().collect();
        sources.sort_by(|a, b| a.path.cmp(&b.path));
        for source in sources {
            db.add_file(kinds, source);
        }
        db.revalidate(kinds);
        db
    }

    fn add_file(&mut self, kinds: &ContentKinds, source: &Source) {
        let path = source.path.as_str();
        self.files.insert(path.to_owned(), source.text.clone());
        let kind = match kinds.of_file(path) {
            Ok(kind) => kind,
            Err(why) => return self.file_errors.push(ContentError::in_file(path, why)),
        };
        let defs = match (kind.fns.parse)(&source.text) {
            Ok(defs) => defs,
            Err(failure) => {
                return self.file_errors.push(ContentError {
                    file: Some(path.to_owned()),
                    position: failure.position,
                    kind: failure.id.is_some().then(|| kind.kind.to_owned()),
                    id: failure.id,
                    message: failure.message,
                });
            }
        };
        let table = self.tables.entry(kind.kind).or_default();
        for (id, def) in defs {
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
            table.insert(id, Entry { file: path.to_owned(), def, json });
        }
    }

    /// Runs every definition's [`ContentDef::validate`] against the database as it is.
    fn revalidate(&mut self, kinds: &ContentKinds) {
        let guard = kinds.registry.as_ref().map(|r| r.read());
        let registry = guard.as_deref();
        let mut errors = Vec::new();
        for (kind, table) in &self.tables {
            let Some(info) = kinds.get(kind) else { continue };
            for (id, entry) in table {
                let mut refs = Refs { db: self, kind, id, file: &entry.file, registry, errors: &mut errors };
                (info.fns.validate)(&entry.def, id, &mut refs);
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

    /// The definition `id` of kind `T`.
    pub fn get<T: ContentDef>(&self, id: &str) -> Option<&T> {
        self.tables.get(T::KIND)?.get(id)?.def.downcast_ref::<T>()
    }

    /// Ids of kind `T`, sorted.
    pub fn ids<T: ContentDef>(&self) -> impl Iterator<Item = &str> {
        self.tables.get(T::KIND).into_iter().flat_map(|t| t.keys().map(String::as_str))
    }

    /// Definitions of kind `T` by id, sorted.
    pub fn iter<T: ContentDef>(&self) -> impl Iterator<Item = (&str, &T)> {
        self.tables
            .get(T::KIND)
            .into_iter()
            .flat_map(|t| t.iter().filter_map(|(id, e)| Some((id.as_str(), e.def.downcast_ref::<T>()?))))
    }

    /// The file definition `id` of kind `T` came from: `content/combat/attacks.attack.ron`.
    pub fn file_of<T: ContentDef>(&self, id: &str) -> Option<&str> {
        self.file(T::KIND, id)
    }

    /// Registered kinds, sorted (only after loading).
    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tables.keys().copied()
    }

    /// Ids of `kind`, sorted; `None` for a kind nobody registered.
    pub fn ids_of(&self, kind: &str) -> Option<Vec<&str>> {
        self.tables.get(kind).map(|t| t.keys().map(String::as_str).collect())
    }

    /// The definition as JSON, for tools and other untyped readers.
    pub fn get_json(&self, kind: &str, id: &str) -> Option<&Value> {
        self.tables.get(kind)?.get(id).map(|e| &e.json)
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
            let table = db.tables.entry(info.kind).or_default();
            match &change.def {
                Some(json) => {
                    let what = |e: String| format!("{} `{}`: {e}", change.kind, change.id);
                    let def = (info.fns.from_json)(json).map_err(what)?;
                    let file = change.file.clone().or_else(|| table.get(&change.id).map(|e| e.file.clone())).unwrap_or_default();
                    let json = (info.fns.to_json)(&def).map_err(what)?;
                    table.insert(change.id.clone(), Entry { file, def, json });
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
    let db = ContentDb::build(kinds, &sources);
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
}
