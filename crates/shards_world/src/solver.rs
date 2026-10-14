//! The socket solver: fills a grid with turned tiles whose edges match, WFC-style.
//!
//! Every cell starts with the options (tile × allowed turn) the steps before left it.
//! Constraint propagation removes from each neighbor what cannot face what remains (AC-3
//! over edge signatures), and count limits remove a tile everywhere once it reached its
//! maximum. The cell with the fewest options left is decided next (ties go to the lowest
//! row-major index), its option drawn by weight from [`DetRng`]; a contradiction undoes
//! the last decision and forbids it, up to a bounded number of backtracks. Every change to a
//! domain is logged, so a decision costs memory for what it changed, not for the grid.
//!
//! Only integers decide: weights are integer milli-units, the order is option counts, so
//! the same seed gives the same tiles on every platform.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use ai_core::DetRng;

use crate::content::TileDef;
use crate::grid::{turns_of, Dir};
use crate::sockets::{Rules, Socket};

/// A tile ready for the solver.
#[derive(Clone, Debug)]
pub struct TileInfo {
    pub id: String,
    /// Edges before turning, N, E, S, W.
    pub edges: [Vec<Socket>; 4],
    /// Allowed quarter turns, ascending.
    pub turns: Vec<u8>,
    /// Weight in thousandths.
    pub weight: u64,
    pub min_count: u32,
    pub max_count: Option<u32>,
    pub tags: Vec<String>,
}

impl TileInfo {
    pub fn from_def(id: &str, def: &TileDef) -> Result<Self, String> {
        let edges = def.sockets.parse().map_err(|e| format!("tile `{id}`: {e}"))?;
        let mut turns: Vec<u8> = def
            .rotations
            .iter()
            .map(|r| turns_of(*r).ok_or_else(|| format!("tile `{id}`: rotation {r} is not 0, 90, 180 or 270")))
            .collect::<Result<_, _>>()?;
        turns.sort_unstable();
        turns.dedup();
        let weight = if def.weight.is_finite() && def.weight > 0.0 { (def.weight * 1000.0).round() as u64 } else { 0 };
        Ok(Self { id: id.to_owned(), edges, turns, weight, min_count: def.min_count, max_count: def.max_count, tags: def.tags.clone() })
    }
}

/// One way to fill a cell: a tile and its quarter turns clockwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileOption {
    pub tile: usize,
    pub turns: u8,
}

/// A tile set compiled for one set of socket rules: options, edge signatures and which
/// may face which.
#[derive(Clone, Debug)]
pub struct TileSet {
    pub rules: Rules,
    pub tiles: Vec<TileInfo>,
    pub options: Vec<TileOption>,
    /// `u64` words per domain.
    pub words: usize,
    weights: Vec<u64>,
    /// Distinct edges, clockwise.
    keys: Vec<Vec<Socket>>,
    /// Edge key of each option per side.
    opt_keys: Vec<[usize; 4]>,
    /// Side → key → options with that key on that side.
    support: [Vec<Vec<u64>>; 4],
    /// Side → key → options that may be the neighbor on that side.
    compat: [Vec<Vec<u64>>; 4],
}

impl TileSet {
    pub fn new(rules: Rules, tiles: Vec<TileInfo>) -> Self {
        let mut options = Vec::new();
        let mut weights = Vec::new();
        for (t, tile) in tiles.iter().enumerate() {
            let share = tile.weight * 12 / tile.turns.len().max(1) as u64;
            for &turns in &tile.turns {
                options.push(TileOption { tile: t, turns });
                weights.push(share);
            }
        }
        let words = options.len().div_ceil(64).max(1);
        let mut keys: Vec<Vec<Socket>> = Vec::new();
        let mut key_of: BTreeMap<Vec<Socket>, usize> = BTreeMap::new();
        let mut opt_keys = Vec::with_capacity(options.len());
        for option in &options {
            let mut sides = [0; 4];
            for dir in Dir::ALL {
                let edge = &tiles[option.tile].edges[(dir.index() + 4 - option.turns as usize) % 4];
                sides[dir.index()] = *key_of.entry(edge.clone()).or_insert_with(|| {
                    keys.push(edge.clone());
                    keys.len() - 1
                });
            }
            opt_keys.push(sides);
        }
        let empty = || vec![vec![0u64; words]; keys.len()];
        let mut support = [empty(), empty(), empty(), empty()];
        for (o, sides) in opt_keys.iter().enumerate() {
            for dir in Dir::ALL {
                set(&mut support[dir.index()][sides[dir.index()]], o);
            }
        }
        let matches: Vec<Vec<bool>> = keys.iter().map(|a| keys.iter().map(|b| rules.edges_match(a, b)).collect()).collect();
        let mut compat = [empty(), empty(), empty(), empty()];
        for dir in Dir::ALL {
            let opposite = dir.opposite().index();
            for k in 0..keys.len() {
                for k2 in 0..keys.len() {
                    if matches[k][k2] {
                        let (target, source) = (&mut compat[dir.index()][k], &support[opposite][k2]);
                        or_assign(target, source);
                    }
                }
            }
        }
        Self { rules, tiles, options, words, weights, keys, opt_keys, support, compat }
    }

    /// The edge of `option` on side `dir`, clockwise.
    pub fn edge(&self, option: usize, dir: Dir) -> &[Socket] {
        &self.keys[self.opt_keys[option][dir.index()]]
    }

    pub fn tile_of(&self, option: usize) -> &TileInfo {
        &self.tiles[self.options[option].tile]
    }

    /// The index of tile `id`.
    pub fn tile_index(&self, id: &str) -> Option<usize> {
        self.tiles.iter().position(|t| t.id == id)
    }

    /// The option of `tile` turned `turns`.
    pub fn option(&self, tile: usize, turns: u8) -> Option<usize> {
        self.options.iter().position(|o| o.tile == tile && o.turns == turns)
    }

    /// Whether `a` may have `b` as its neighbor on side `dir`.
    pub fn fits(&self, a: usize, dir: Dir, b: usize) -> bool {
        has(&self.compat[dir.index()][self.opt_keys[a][dir.index()]], b)
    }

    /// A domain holding the options for which `keep` is true.
    pub fn bits_where(&self, keep: impl Fn(usize, &TileOption) -> bool) -> Vec<u64> {
        let mut bits = vec![0u64; self.words];
        for (o, option) in self.options.iter().enumerate() {
            if keep(o, option) {
                set(&mut bits, o);
            }
        }
        bits
    }

    /// Options whose side `dir` may face a neighbor edge `edge` (a region border).
    pub fn facing_bits(&self, dir: Dir, edge: &[Socket]) -> Vec<u64> {
        self.bits_where(|o, _| self.rules.edges_match(self.edge(o, dir), edge))
    }

    /// The option weight (tile weight shared by its turns, ×12 so every share is whole).
    pub fn weight(&self, option: usize) -> u64 {
        self.weights[option]
    }
}

// --- bit sets ----------------------------------------------------------------------------

pub(crate) fn set(bits: &mut [u64], i: usize) {
    bits[i / 64] |= 1 << (i % 64);
}

pub(crate) fn has(bits: &[u64], i: usize) -> bool {
    bits[i / 64] & (1 << (i % 64)) != 0
}

pub(crate) fn count(bits: &[u64]) -> u32 {
    bits.iter().map(|w| w.count_ones()).sum()
}

pub(crate) fn is_empty(bits: &[u64]) -> bool {
    bits.iter().all(|w| *w == 0)
}

pub(crate) fn intersects(a: &[u64], b: &[u64]) -> bool {
    a.iter().zip(b).any(|(x, y)| x & y != 0)
}

/// `a ⊆ b`.
pub(crate) fn subset(a: &[u64], b: &[u64]) -> bool {
    a.iter().zip(b).all(|(x, y)| x & !y == 0)
}

pub(crate) fn or_assign(a: &mut [u64], b: &[u64]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x |= y;
    }
}

/// `a &= b`; whether `a` changed.
pub(crate) fn and_assign(a: &mut [u64], b: &[u64]) -> bool {
    let mut changed = false;
    for (x, y) in a.iter_mut().zip(b) {
        let new = *x & y;
        changed |= new != *x;
        *x = new;
    }
    changed
}

/// `a &= !b`; whether `a` changed.
pub(crate) fn and_not_assign(a: &mut [u64], b: &[u64]) -> bool {
    let mut changed = false;
    for (x, y) in a.iter_mut().zip(b) {
        let new = *x & !y;
        changed |= new != *x;
        *x = new;
    }
    changed
}

pub(crate) fn ones(bits: &[u64]) -> impl Iterator<Item = usize> + '_ {
    bits.iter().enumerate().flat_map(|(w, word)| (0..64).filter(move |b| word & (1 << b) != 0).map(move |b| w * 64 + b))
}

// --- solving -----------------------------------------------------------------------------

/// A count limit: options of one tile or tag, at least `min`, at most `max` cells.
#[derive(Clone, Debug)]
pub struct LimitBits {
    /// `tile ruin`, `#village`.
    pub name: String,
    pub bits: Vec<u64>,
    pub min: u32,
    pub max: Option<u32>,
}

/// Why the solver found no tiling.
#[derive(Clone, Debug, PartialEq)]
pub enum SolveError {
    /// The constraints alone leave nothing that fits at `cell` (row-major index).
    Empty { cell: usize },
    /// The constraints alone break a count limit.
    Limit { name: String, min: u32, max: Option<u32>, possible: u32, certain: u32 },
    /// Every choice was tried.
    Exhausted { backtracks: u32 },
    /// The backtrack budget ran out.
    GaveUp { backtracks: u32 },
}

impl fmt::Display for SolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolveError::Empty { cell } => write!(f, "no tile fits cell #{cell}"),
            SolveError::Limit { name, min, max, possible, certain } => {
                let max = max.map_or_else(|| "any".to_owned(), |m| m.to_string());
                write!(f, "{name} must appear {min}..{max} times, but {certain} cells must and at most {possible} can hold it")
            }
            SolveError::Exhausted { backtracks } => write!(f, "no tiling exists for these constraints ({backtracks} backtracks)"),
            SolveError::GaveUp { backtracks } => write!(f, "gave up after {backtracks} backtracks"),
        }
    }
}

/// A finished tiling.
#[derive(Clone, Debug, PartialEq)]
pub struct Solution {
    /// The option in every cell, row-major.
    pub options: Vec<usize>,
    pub choices: u32,
    pub backtracks: u32,
}

enum Conflict {
    Cell(usize),
    Limit(usize, u32, u32),
}

/// One grid to fill.
pub struct Solver<'a> {
    pub set: &'a TileSet,
    pub width: usize,
    pub height: usize,
    pub limits: &'a [LimitBits],
    pub max_backtracks: u32,
}

/// A decision: the trail as it was (`mark`), the cell and the option drawn for it.
struct Frame {
    mark: usize,
    cell: usize,
    option: usize,
}

/// The domains' changes in order, each as the cell and its `words` before the change, so that
/// undoing a decision puts back what it and its consequences changed.
struct Trail {
    words: usize,
    cells: Vec<usize>,
    saved: Vec<u64>,
}

impl Trail {
    fn new(words: usize) -> Self {
        Self { words, cells: Vec::new(), saved: Vec::new() }
    }

    fn len(&self) -> usize {
        self.cells.len()
    }

    /// Logs `cell`'s domain before a change.
    fn save(&mut self, cell: usize, d: &[u64]) {
        self.cells.push(cell);
        self.saved.extend_from_slice(&d[cell * self.words..(cell + 1) * self.words]);
    }

    /// Takes the last [`save`](Self::save) back: nothing changed after all.
    fn unsave(&mut self) {
        self.cells.pop();
        self.saved.truncate(self.saved.len() - self.words);
    }

    /// Puts back every domain changed since the trail was `mark` long, newest first.
    fn undo(&mut self, mark: usize, d: &mut [u64]) {
        let w = self.words;
        while self.cells.len() > mark {
            let cell = self.cells.pop().unwrap_or_default();
            let from = self.saved.len() - w;
            d[cell * w..(cell + 1) * w].copy_from_slice(&self.saved[from..]);
            self.saved.truncate(from);
        }
    }

    fn clear(&mut self) {
        self.cells.clear();
        self.saved.clear();
    }
}

impl Solver<'_> {
    fn cells(&self) -> usize {
        self.width * self.height
    }

    /// Fills the grid from `domains` (the options left in every cell, row-major, `words`
    /// per cell). Deterministic for the same `rng`.
    pub fn solve(&self, mut domains: Vec<u64>, rng: &mut DetRng) -> Result<Solution, SolveError> {
        let w = self.set.words;
        assert_eq!(domains.len(), self.cells() * w, "one domain per cell");
        let mut queue: VecDeque<usize> = (0..self.cells()).collect();
        let mut queued = vec![true; self.cells()];
        let mut stack: Vec<Frame> = Vec::new();
        let mut trail = Trail::new(w);
        let (mut choices, mut backtracks) = (0u32, 0u32);
        loop {
            match self.settle(&mut domains, &mut queue, &mut queued, &mut trail) {
                Ok(()) => {
                    let Some(cell) = self.pick_cell(&domains) else {
                        let options = (0..self.cells()).map(|c| ones(&domains[c * w..(c + 1) * w]).next().unwrap_or(0)).collect();
                        return Ok(Solution { options, choices, backtracks });
                    };
                    let option = self.choose(&domains[cell * w..(cell + 1) * w], rng);
                    if stack.is_empty() {
                        // Nothing before the first decision is ever taken back.
                        trail.clear();
                    }
                    stack.push(Frame { mark: trail.len(), cell, option });
                    trail.save(cell, &domains);
                    let dom = &mut domains[cell * w..(cell + 1) * w];
                    dom.fill(0);
                    set(dom, option);
                    choices += 1;
                    self.enqueue(cell, &mut queue, &mut queued);
                }
                Err(conflict) => {
                    let Some(frame) = stack.pop() else {
                        if backtracks > 0 {
                            return Err(SolveError::Exhausted { backtracks });
                        }
                        return Err(match conflict {
                            Conflict::Cell(cell) => SolveError::Empty { cell },
                            Conflict::Limit(l, possible, certain) => {
                                let limit = &self.limits[l];
                                SolveError::Limit { name: limit.name.clone(), min: limit.min, max: limit.max, possible, certain }
                            }
                        });
                    };
                    backtracks += 1;
                    if backtracks > self.max_backtracks {
                        return Err(SolveError::GaveUp { backtracks: self.max_backtracks });
                    }
                    // Back to before the choice, without it. The cell had two options or
                    // more, so one is left. Dropping it is logged too: it holds only while
                    // the decision before this one does.
                    trail.undo(frame.mark, &mut domains);
                    trail.save(frame.cell, &domains);
                    let bit = 1u64 << (frame.option % 64);
                    domains[frame.cell * w + frame.option / 64] &= !bit;
                    queue.clear();
                    queued.fill(false);
                    self.enqueue(frame.cell, &mut queue, &mut queued);
                }
            }
        }
    }

    fn enqueue(&self, cell: usize, queue: &mut VecDeque<usize>, queued: &mut [bool]) {
        if !queued[cell] {
            queued[cell] = true;
            queue.push_back(cell);
        }
    }

    /// Propagation and limits until nothing changes.
    fn settle(&self, d: &mut [u64], queue: &mut VecDeque<usize>, queued: &mut [bool], trail: &mut Trail) -> Result<(), Conflict> {
        loop {
            self.propagate(d, queue, queued, trail)?;
            if !self.apply_limits(d, queue, queued, trail)? {
                return Ok(());
            }
        }
    }

    fn propagate(&self, d: &mut [u64], queue: &mut VecDeque<usize>, queued: &mut [bool], trail: &mut Trail) -> Result<(), Conflict> {
        let w = self.set.words;
        let mut allowed = vec![0u64; w];
        while let Some(c) = queue.pop_front() {
            queued[c] = false;
            let (x, y) = ((c % self.width) as i32, (c / self.width) as i32);
            for dir in Dir::ALL {
                let (dx, dy) = dir.offset();
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= self.width as i32 || ny >= self.height as i32 {
                    continue;
                }
                let n = ny as usize * self.width + nx as usize;
                allowed.fill(0);
                {
                    let dom = &d[c * w..(c + 1) * w];
                    for (k, support) in self.set.support[dir.index()].iter().enumerate() {
                        if intersects(dom, support) {
                            or_assign(&mut allowed, &self.set.compat[dir.index()][k]);
                        }
                    }
                }
                trail.save(n, d);
                let neighbor = &mut d[n * w..(n + 1) * w];
                if and_assign(neighbor, &allowed) {
                    if is_empty(neighbor) {
                        return Err(Conflict::Cell(n));
                    }
                    self.enqueue(n, queue, queued);
                } else {
                    trail.unsave();
                }
            }
        }
        Ok(())
    }

    /// Checks every limit; a limit at its maximum is removed from the cells not bound to
    /// it. Returns whether a domain changed.
    fn apply_limits(&self, d: &mut [u64], queue: &mut VecDeque<usize>, queued: &mut [bool], trail: &mut Trail) -> Result<bool, Conflict> {
        let w = self.set.words;
        let mut changed = false;
        for (l, limit) in self.limits.iter().enumerate() {
            let (mut possible, mut certain) = (0u32, 0u32);
            for c in 0..self.cells() {
                let dom = &d[c * w..(c + 1) * w];
                if intersects(dom, &limit.bits) {
                    possible += 1;
                    certain += u32::from(subset(dom, &limit.bits));
                }
            }
            if limit.max.is_some_and(|max| certain > max) || possible < limit.min {
                return Err(Conflict::Limit(l, possible, certain));
            }
            if limit.max == Some(certain) && possible > certain {
                for c in 0..self.cells() {
                    if subset(&d[c * w..(c + 1) * w], &limit.bits) {
                        continue;
                    }
                    trail.save(c, d);
                    if and_not_assign(&mut d[c * w..(c + 1) * w], &limit.bits) {
                        changed = true;
                        self.enqueue(c, queue, queued);
                    } else {
                        trail.unsave();
                    }
                }
            }
        }
        Ok(changed)
    }

    /// The undecided cell with the fewest options; ties go to the lowest index.
    fn pick_cell(&self, d: &[u64]) -> Option<usize> {
        let w = self.set.words;
        let mut best: Option<(u32, usize)> = None;
        for c in 0..self.cells() {
            let n = count(&d[c * w..(c + 1) * w]);
            if n > 1 && best.is_none_or(|(b, _)| n < b) {
                best = Some((n, c));
            }
        }
        best.map(|(_, c)| c)
    }

    /// An option of `dom`, drawn by weight (the first when all weigh nothing).
    fn choose(&self, dom: &[u64], rng: &mut DetRng) -> usize {
        let total: u64 = ones(dom).map(|o| self.set.weight(o)).sum();
        let first = ones(dom).next().unwrap_or(0);
        if total == 0 {
            return first;
        }
        let pick = ((rng.next_u64() as u128 * total as u128) >> 64) as u64;
        let mut acc = 0;
        for o in ones(dom) {
            acc += self.set.weight(o);
            if pick < acc {
                return o;
            }
        }
        first
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    use super::*;
    use crate::content::{SocketRulesDef, Sockets};

    /// Counts the bytes alive and their peak, to see how much memory a solve holds.
    struct Counting;

    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    // SAFETY: every call goes to the system allocator unchanged; the counters only observe.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let now = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(now, Relaxed);
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size(), Relaxed);
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    fn rules() -> Rules {
        Rules::new(&SocketRulesDef {
            kinds: ["a", "b", "x"].map(String::from).to_vec(),
            pairs: vec![],
            forbid: vec!["x".into()],
            any_height: vec![],
            height_tolerance: 0,
            passable: vec!["a".into()],
            roads: vec!["b".into()],
        })
    }

    fn tile(id: &str, [n, e, s, w]: [&str; 4], rotations: &[u16]) -> TileInfo {
        let def = TileDef {
            sockets: Sockets { n: n.into(), e: e.into(), s: s.into(), w: w.into() },
            rotations: rotations.to_vec(),
            weight: 1.0,
            min_count: 0,
            max_count: None,
            tags: vec![],
            pois: vec![],
            slots: vec![],
            links: None,
            slope: 0,
            decor: vec![],
            geometry: Default::default(),
            visual: None,
        };
        TileInfo::from_def(id, &def).unwrap()
    }

    fn solver<'a>(set: &'a TileSet, w: usize, h: usize, limits: &'a [LimitBits]) -> Solver<'a> {
        Solver { set, width: w, height: h, limits, max_backtracks: 500 }
    }

    fn full(set: &TileSet, cells: usize) -> Vec<u64> {
        let all = set.bits_where(|_, _| true);
        (0..cells).flat_map(|_| all.clone()).collect()
    }

    /// Every pair of neighbors in `options` fits.
    fn assert_matches(set: &TileSet, w: usize, h: usize, options: &[usize]) {
        for y in 0..h {
            for x in 0..w {
                let o = options[y * w + x];
                if x + 1 < w {
                    let e = options[y * w + x + 1];
                    assert!(set.rules.edges_match(set.edge(o, Dir::E), set.edge(e, Dir::W)), "({x},{y}) E");
                }
                if y + 1 < h {
                    let s = options[(y + 1) * w + x];
                    assert!(set.rules.edges_match(set.edge(o, Dir::S), set.edge(s, Dir::N)), "({x},{y}) S");
                }
            }
        }
    }

    #[test]
    fn turned_edges_move_clockwise_and_keep_their_order() {
        let set = TileSet::new(rules(), vec![tile("t", ["a b a", "a", "b", "a"], &[0, 90, 180, 270])]);
        assert_eq!(set.options.len(), 4);
        let turned = set.option(0, 1).unwrap();
        // The north edge faces east after a quarter turn; the south one faces west.
        assert_eq!(crate::sockets::edge_text(set.edge(turned, Dir::E)), "a b a");
        assert_eq!(crate::sockets::edge_text(set.edge(turned, Dir::W)), "b");
        assert_eq!(set.weight(turned) * 4, 12_000, "turns share the tile's weight");
    }

    #[test]
    fn solutions_match_everywhere_and_repeat_for_the_same_key() {
        let set = TileSet::new(
            rules(),
            vec![
                tile("plain", ["a", "a", "a", "a"], &[0]),
                tile("straight", ["b", "a", "b", "a"], &[0, 90]),
                tile("bend", ["b", "b", "a", "a"], &[0, 90, 180, 270]),
                tile("cross", ["b", "b", "b", "b"], &[0]),
            ],
        );
        let (w, h) = (6, 5);
        let s = solver(&set, w, h, &[]);
        let one = s.solve(full(&set, w * h), &mut DetRng::from_keys(&[1, 2])).unwrap();
        assert_matches(&set, w, h, &one.options);
        let again = s.solve(full(&set, w * h), &mut DetRng::from_keys(&[1, 2])).unwrap();
        assert_eq!(one, again, "deterministic");
        let other = s.solve(full(&set, w * h), &mut DetRng::from_keys(&[1, 3])).unwrap();
        assert_ne!(one.options, other.options, "another key, another tiling");
    }

    #[test]
    fn fixed_cells_and_forbidden_borders_hold() {
        let set = TileSet::new(
            rules(),
            vec![tile("plain", ["a", "a", "a", "a"], &[0]), tile("end", ["b", "a", "a", "a"], &[0, 90, 180, 270]), tile("straight", ["b", "a", "b", "a"], &[0, 90])],
        );
        let (w, h) = (4, 1);
        let mut d = full(&set, w * h);
        let words = set.words;
        // A road must leave cell 0 to the east: its option is `end` turned to face E.
        let end_east = set.option(1, 1).unwrap();
        d[..words].fill(0);
        set_bit(&mut d[..words], end_east);
        let solution = solver(&set, w, h, &[]).solve(d, &mut DetRng::new(9)).unwrap();
        assert_eq!(solution.options[0], end_east);
        assert_matches(&set, w, h, &solution.options);
        // The road runs on: cell 1 must take it from the west.
        assert!(crate::sockets::edge_text(set.edge(solution.options[1], Dir::W)) == "b");
    }

    fn set_bit(bits: &mut [u64], i: usize) {
        set(bits, i);
    }

    #[test]
    fn limits_cap_and_force_counts() {
        let set = TileSet::new(rules(), vec![tile("plain", ["a", "a", "a", "a"], &[0]), tile("rock", ["a", "a", "a", "a"], &[0])]);
        let rock = set.bits_where(|_, o| o.tile == 1);
        let (w, h) = (5, 5);
        let capped = [LimitBits { name: "tile rock".into(), bits: rock.clone(), min: 0, max: Some(2) }];
        for seed in 0..20 {
            let s = solver(&set, w, h, &capped).solve(full(&set, w * h), &mut DetRng::new(seed)).unwrap();
            assert!(s.options.iter().filter(|&&o| o == 1).count() <= 2);
        }
        let exact = [LimitBits { name: "tile rock".into(), bits: rock, min: 3, max: Some(3) }];
        for seed in 0..20 {
            let s = solver(&set, w, h, &exact).solve(full(&set, w * h), &mut DetRng::new(seed)).unwrap();
            assert_eq!(s.options.iter().filter(|&&o| o == 1).count(), 3, "seed {seed}");
        }
    }

    #[test]
    fn impossible_constraints_say_where() {
        let set = TileSet::new(rules(), vec![tile("plain", ["a", "a", "a", "a"], &[0]), tile("road", ["b", "b", "b", "b"], &[0])]);
        let (w, h) = (2, 1);
        let mut d = full(&set, w * h);
        let words = set.words;
        d[..words].copy_from_slice(&set.bits_where(|_, o| o.tile == 0));
        d[words..].copy_from_slice(&set.bits_where(|_, o| o.tile == 1));
        let err = solver(&set, w, h, &[]).solve(d, &mut DetRng::new(1)).unwrap_err();
        assert!(matches!(err, SolveError::Empty { .. }), "{err}");
    }

    #[test]
    fn borders_narrow_the_cells_along_them() {
        let set = TileSet::new(
            rules(),
            vec![tile("l", ["a", "b", "a", "a"], &[0]), tile("m", ["a", "b", "a", "b"], &[0]), tile("r", ["a", "a", "a", "b"], &[0])],
        );
        let (w, h) = (3, 1);
        let mut d = full(&set, w * h);
        let words = set.words;
        let sealed = [Socket { kind: "a".into(), height: 0 }];
        and_assign(&mut d[..words], &set.facing_bits(Dir::W, &sealed));
        and_assign(&mut d[2 * words..], &set.facing_bits(Dir::E, &sealed));
        let s = solver(&set, w, h, &[]).solve(d, &mut DetRng::new(3)).unwrap();
        let ids: Vec<&str> = s.options.iter().map(|&o| set.tile_of(o).id.as_str()).collect();
        assert_eq!(ids, ["l", "m", "r"]);
    }

    #[test]
    fn a_choice_costs_memory_for_what_it_changed_not_for_the_whole_grid() {
        // Two tiles that fit anywhere: every cell of a 64×64 grid is decided, one choice each.
        // A copy of the grid per choice would hold 4096 × 4096 words, over 100 MB.
        let set = TileSet::new(rules(), vec![tile("a", ["a", "a", "a", "a"], &[0]), tile("b", ["a", "a", "a", "a"], &[0])]);
        let (w, h) = (64, 64);
        let domains = full(&set, w * h);
        let before = LIVE.load(Relaxed);
        PEAK.store(before, Relaxed);
        let solved = solver(&set, w, h, &[]).solve(domains, &mut DetRng::new(5)).unwrap();
        let held = PEAK.load(Relaxed).saturating_sub(before);
        assert_eq!(solved.choices as usize, w * h);
        assert!(held < 8 << 20, "a solve held {} MiB", held >> 20);
    }

    #[test]
    fn backtracking_undoes_choices_that_lead_nowhere() {
        // Rocks may not touch each other, and a 1×3 strip needs exactly two: only
        // `rock plain rock` works, and heavy plain tiles get chosen first and undone.
        let rules = Rules::new(&SocketRulesDef {
            kinds: ["a", "p", "r", "s"].map(String::from).to_vec(),
            pairs: vec![("r".into(), "p".into()), ("s".into(), "p".into())],
            forbid: vec![],
            any_height: vec![],
            height_tolerance: 0,
            passable: vec![],
            roads: vec![],
        });
        let mut plain = tile("plain", ["a", "p", "a", "p"], &[0]);
        plain.weight = 100_000;
        let set = TileSet::new(rules, vec![plain, tile("rock", ["a", "r", "a", "s"], &[0])]);
        let rock = set.bits_where(|_, o| o.tile == 1);
        let limits = [LimitBits { name: "tile rock".into(), bits: rock, min: 2, max: Some(2) }];
        let (w, h) = (3, 1);
        let mut backtracked = 0;
        for seed in 0..10 {
            let s = solver(&set, w, h, &limits).solve(full(&set, w * h), &mut DetRng::new(seed)).unwrap();
            let ids: Vec<&str> = s.options.iter().map(|&o| set.tile_of(o).id.as_str()).collect();
            assert_eq!(ids, ["rock", "plain", "rock"], "seed {seed}");
            backtracked += s.backtracks;
        }
        assert!(backtracked > 0, "the heavy wrong choices were undone");
        // One rock too many for the strip: every choice fails.
        let limits = [LimitBits { name: "tile rock".into(), bits: set.bits_where(|_, o| o.tile == 1), min: 3, max: None }];
        let err = solver(&set, w, h, &limits).solve(full(&set, w * h), &mut DetRng::new(1)).unwrap_err();
        assert!(matches!(err, SolveError::Exhausted { .. } | SolveError::Limit { .. }), "{err}");
    }
}
