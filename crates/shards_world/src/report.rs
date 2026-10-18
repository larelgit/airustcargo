//! Batch checks without a window: generate a region for many seeds and report.
//!
//! The `worldgen_report` tool, the game binary's `--worldgen-report REGION --seeds 1..1000`
//! and `shards worldgen --region R --seeds 1..1000 --report` all print this: how many seeds
//! passed (and how many needed another attempt), how many fell back or failed, failed
//! attempts grouped by reason with example seeds, errors that stopped a seed with no attempt to
//! blame (a broken fallback), distances between points of interest
//! (min / median / max per pair of types), and generator panics with their seed.
//!
//! ```text
//! ok: worldgen meadow v1, seeds 1..1000: 1000/1000 passed (41 after a retry), 0 fell back, 0 failed, 0 panicked [1000 seeds in 1.84s, 1.8 ms/seed]
//! failed attempts (52):
//!   31× check reachable (e.g. seeds 17, 88, 203)
//!   21× fill: no tile fits (e.g. seeds 5, 9, 12)
//! distances between points (m, min / median / max):
//!   camp–camp  33.9 / 67.9 / 135.8
//!   camp–spawn  32.0 / 56.6 / 113.1
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use shards_content::ContentDb;

use crate::content::RegionDef;
use crate::generate::{self, unknown_region_of, GenError, Generated, WorldGen};
use crate::manifest::RegionManifest;

/// How seeds are written, for errors that teach it.
pub const SEEDS_SYNTAX: &str = "seeds are `1..1000` (both ends included), `7`, or a list such as `1,5,10..20`";

/// The most seeds one report runs.
pub const MAX_SEEDS: u64 = 1_000_000;

/// Parses `1..1000` (both ends included), `1..=1000`, `7` or a comma-separated list of those.
pub fn parse_seeds(text: &str) -> Result<Vec<u64>, String> {
    let mut seeds = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let number = |s: &str| s.trim().parse::<u64>().map_err(|_| format!("`{part}`: `{}` is not a seed; {SEEDS_SYNTAX}", s.trim()));
        match part.split_once("..") {
            Some((from, to)) => {
                let (from, to) = (number(from)?, number(to.trim_start_matches('='))?);
                if to < from {
                    return Err(format!("`{part}` runs backwards; {SEEDS_SYNTAX}"));
                }
                if to - from >= MAX_SEEDS || seeds.len() as u64 + (to - from) >= MAX_SEEDS {
                    return Err(format!("`{part}` is more than {MAX_SEEDS} seeds"));
                }
                seeds.extend(from..=to);
            }
            None => seeds.push(number(part)?),
        }
    }
    if seeds.is_empty() {
        return Err(format!("no seeds; {SEEDS_SYNTAX}"));
    }
    Ok(seeds)
}

/// Failed attempts of one reason.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reason {
    pub count: u32,
    /// Up to three seeds where it happened.
    pub examples: Vec<u64>,
}

/// What a batch found.
#[derive(Clone, Debug, PartialEq)]
pub struct BatchReport {
    pub region: String,
    pub version: u32,
    /// As asked for: `1..1000`.
    pub seeds_text: String,
    pub seeds: usize,
    /// Seeds whose attempts produced a region that passed every check.
    pub passed: usize,
    /// Of those, the ones that needed more than one attempt.
    pub retried: usize,
    /// Seeds that ended with the fallback tile map.
    pub fell_back: Vec<u64>,
    /// Seeds with no region at all.
    pub failed: Vec<u64>,
    /// Seeds whose generator panicked, with the message.
    pub panics: Vec<(u64, String)>,
    /// Failed attempts by reason (`check reachable`, `fill: no tile fits`).
    pub reasons: BTreeMap<String, Reason>,
    /// Why seeds got no region when their attempts do not say: the fallback broke, content
    /// names a room it lacks. The error's first line, with example seeds.
    pub errors: BTreeMap<String, Reason>,
    /// Distance (m) from each point of interest to the nearest of another type (or another
    /// of its own type), per pair `a–b`, over the passing regions.
    pub distances: BTreeMap<String, Vec<f32>>,
    /// What the region kind counted, summed over the seeds that made a region (`solid
    /// cells`, `water boxes`; empty for tiles and rooms).
    pub counts: BTreeMap<String, u64>,
    pub elapsed: Duration,
}

impl BatchReport {
    /// Every seed passed.
    pub fn all_passed(&self) -> bool {
        self.passed == self.seeds
    }

    /// `(min, median, max)` of pair `a–b`.
    pub fn distance_stats(&self, pair: &str) -> Option<(f32, f32, f32)> {
        let mut d = self.distances.get(pair)?.clone();
        if d.is_empty() {
            return None;
        }
        d.sort_by(f32::total_cmp);
        Some((d[0], d[d.len() / 2], d[d.len() - 1]))
    }
}

impl fmt::Display for BatchReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = if self.all_passed() { "ok" } else { "FAILED" };
        let secs = self.elapsed.as_secs_f64();
        let per = if self.seeds > 0 { secs * 1000.0 / self.seeds as f64 } else { 0.0 };
        writeln!(
            f,
            "{verdict}: worldgen {} v{}, seeds {}: {}/{} passed ({} after a retry), {} fell back, {} failed, {} panicked [{} seeds in {secs:.2}s, {per:.1} ms/seed]",
            self.region,
            self.version,
            self.seeds_text,
            self.passed,
            self.seeds,
            self.retried,
            self.fell_back.len(),
            self.failed.len(),
            self.panics.len(),
            self.seeds
        )?;
        let seeds = |list: &[u64]| list.iter().take(10).map(ToString::to_string).collect::<Vec<_>>().join(", ");
        if !self.fell_back.is_empty() {
            writeln!(f, "fell back: seeds {}", seeds(&self.fell_back))?;
        }
        if !self.failed.is_empty() {
            writeln!(f, "failed: seeds {}", seeds(&self.failed))?;
        }
        if !self.reasons.is_empty() {
            let total: u32 = self.reasons.values().map(|r| r.count).sum();
            writeln!(f, "failed attempts ({total}):")?;
            let mut reasons: Vec<(&String, &Reason)> = self.reasons.iter().collect();
            reasons.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
            for (rule, reason) in reasons {
                writeln!(f, "  {}× {rule} (e.g. seeds {})", reason.count, seeds(&reason.examples))?;
            }
        }
        if !self.errors.is_empty() {
            let total: u32 = self.errors.values().map(|r| r.count).sum();
            writeln!(f, "errors ({total}):")?;
            for (message, reason) in &self.errors {
                writeln!(f, "  {}× {message} (e.g. seeds {})", reason.count, seeds(&reason.examples))?;
            }
        }
        if !self.counts.is_empty() {
            let made = (self.passed + self.fell_back.len()).max(1) as u64;
            let items: Vec<String> = self.counts.iter().map(|(what, n)| format!("{what} {n} ({} a region)", n / made)).collect();
            writeln!(f, "totals: {}", items.join(", "))?;
        }
        if !self.distances.is_empty() {
            writeln!(f, "distances between points (m, min / median / max):")?;
            for pair in self.distances.keys() {
                if let Some((min, median, max)) = self.distance_stats(pair) {
                    writeln!(f, "  {pair}  {min:.1} / {median:.1} / {max:.1}")?;
                }
            }
        }
        if !self.panics.is_empty() {
            writeln!(f, "panics:")?;
            for (seed, message) in &self.panics {
                writeln!(f, "  seed {seed}: {message}")?;
            }
        }
        Ok(())
    }
}

/// Generates `region` for every seed and reports. Panics in the generator (a game's step,
/// say) are caught per seed. `Err` only for a region that does not exist or does not
/// compile.
pub fn run(db: &ContentDb, worldgen: &WorldGen, region: &str, seeds_text: &str) -> Result<BatchReport, String> {
    let seeds = parse_seeds(seeds_text)?;
    let started = Instant::now();
    let kind = worldgen.kind_of(db, region);
    let compiled = match db.get::<RegionDef>(region) {
        Some(def) => Some(generate::compile(db, region, def)?),
        None if db.get::<crate::content::DungeonDef>(region).is_some() || kind.is_some() => None,
        None => return Err(unknown_region_of(db, Some(worldgen), region)),
    };
    let version = match (&compiled, kind) {
        (Some(c), _) => c.def.version,
        (None, Some(kind)) if db.get::<crate::content::DungeonDef>(region).is_none() => (kind.version)(db, region).unwrap_or(0),
        (None, _) => db.get::<crate::content::DungeonDef>(region).map_or(0, |d| d.version),
    };
    let mut report = BatchReport {
        region: region.to_owned(),
        version,
        seeds_text: seeds_text.trim().to_owned(),
        seeds: seeds.len(),
        passed: 0,
        retried: 0,
        fell_back: Vec::new(),
        failed: Vec::new(),
        panics: Vec::new(),
        reasons: BTreeMap::new(),
        errors: BTreeMap::new(),
        distances: BTreeMap::new(),
        counts: BTreeMap::new(),
        elapsed: Duration::ZERO,
    };
    for seed in seeds {
        let outcome = catch_unwind(AssertUnwindSafe(|| match &compiled {
            Some(c) => generate::generate_compiled(db, worldgen, c, seed),
            None => generate::generate(db, worldgen, region, seed),
        }));
        match outcome {
            Ok(Ok(Generated { manifest, failures, counts, .. })) => {
                for failure in &failures {
                    tally(&mut report.reasons, &failure.rule, seed);
                }
                for (what, n) in counts {
                    *report.counts.entry(what).or_default() += n;
                }
                if manifest.fallback.is_some() {
                    report.fell_back.push(seed);
                } else {
                    report.passed += 1;
                    report.retried += usize::from(!failures.is_empty());
                    add_distances(&mut report.distances, &manifest);
                }
            }
            Ok(Err(GenError { message, failures })) => {
                for failure in &failures {
                    tally(&mut report.reasons, &failure.rule, seed);
                }
                // "No attempt passed" quotes the last attempt, which is counted above; any
                // other error (the fallback broke, a room is unknown) has no attempt to show.
                if !failures.last().is_some_and(|last| message.contains(&last.message)) {
                    tally(&mut report.errors, message.lines().next().unwrap_or_default(), seed);
                }
                report.failed.push(seed);
            }
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_owned());
                // The report names it with its seed: no crash report.
                ai_core::crash::handled(&message);
                report.panics.push((seed, message));
                report.failed.push(seed);
            }
        }
    }
    report.elapsed = started.elapsed();
    Ok(report)
}

/// Counts `seed` under `key`, keeping up to three example seeds.
fn tally(into: &mut BTreeMap<String, Reason>, key: &str, seed: u64) {
    let reason = into.entry(key.to_owned()).or_default();
    reason.count += 1;
    if reason.examples.len() < 3 && !reason.examples.contains(&seed) {
        reason.examples.push(seed);
    }
}

/// For every pair of point types `a–b` (sorted), each `a`'s distance to the nearest other
/// `b`.
pub fn add_distances(out: &mut BTreeMap<String, Vec<f32>>, manifest: &RegionManifest) {
    let mut kinds: Vec<&str> = manifest.pois.iter().map(|p| p.poi.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    for (i, a) in kinds.iter().enumerate() {
        for b in &kinds[i..] {
            let pair = format!("{a}–{b}");
            for p in manifest.pois.iter().filter(|p| p.poi == *a) {
                let nearest =
                    manifest.pois.iter().filter(|q| q.poi == *b && q.id != p.id).map(|q| p.position().distance(q.position())).fold(f32::INFINITY, f32::min);
                if nearest.is_finite() {
                    out.entry(pair.clone()).or_default().push(nearest);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_ranges_with_both_ends_lists_and_single_numbers() {
        assert_eq!(parse_seeds("1..1000").unwrap().len(), 1000);
        assert_eq!(parse_seeds("1..=3").unwrap(), [1, 2, 3]);
        assert_eq!(parse_seeds(" 7 ").unwrap(), [7]);
        assert_eq!(parse_seeds("1,5, 10..12").unwrap(), [1, 5, 10, 11, 12]);
        assert!(parse_seeds("5..1").unwrap_err().contains("backwards"));
        assert!(parse_seeds("a..b").unwrap_err().contains("both ends included"));
        assert!(parse_seeds("").unwrap_err().starts_with("no seeds"));
        assert!(parse_seeds("0..2000000").unwrap_err().contains("more than"));
    }
}
