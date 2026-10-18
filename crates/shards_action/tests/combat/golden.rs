//! Old content keeps its JSON. Every content file that existed before the hit-outcome
//! refactor (B1) parses and normalizes to the same definitions it did at `988723b`: the
//! content half of a replay check (`ContentDb::hash`) hashes exactly this JSON, so a new
//! field that serialized by default would make every recorded replay diverge at its
//! first check. The list is the hash of each file's definitions; it changes only when a
//! file is edited on purpose (and then the reference replays are recorded again).

use shards_action::{ActionDef, CombatTuning, MovesetDef, StatusDef};
use shards_content::db::Source;
use shards_content::{ContentDb, ContentKinds};

/// `(path from the repository root, ContentDb::hash of the definitions in that file)`.
const GOLDEN: &[(&str, u64)] = &[
    ("crates/sandbox/assets/content/combat/fighters.moveset.ron", 0x3f2d1b75f7ef162b),
    ("crates/sandbox/assets/content/combat/player.action.ron", 0xf72d88299141ee9b),
    ("crates/sandbox/assets/content/combat/skeleton.action.ron", 0x3dc26642fe6753f4),
    ("crates/sandbox/assets/content/combat/tuning.combat_tuning.ron", 0x7fc354d607b29452),
    ("crates/sandbox/assets/content/combat/warden.action.ron", 0x51f46c5018e399fc),
    ("crates/shards_action/tests/combat/assets/content/brute.action.ron", 0xe5fe933d72bed5b7),
    ("crates/shards_action/tests/combat/assets/content/edges.action.ron", 0xe8111b9d9c1a093d),
    ("crates/shards_action/tests/combat/assets/content/edges.moveset.ron", 0x5baedad6f6e785a4),
    ("crates/shards_action/tests/combat/assets/content/fighters.moveset.ron", 0x300884441bc0d391),
    ("crates/shards_action/tests/combat/assets/content/hero.action.ron", 0x697f1622a8e06b37),
    ("crates/shards_action/tests/combat/assets/content/tuning.combat_tuning.ron", 0xdee81563b5407e4f),
    ("crates/shards_npc/tests/npc/assets/content/fighters.action.ron", 0x872071bdaa5f9542),
    ("crates/shards_npc/tests/npc/assets/content/fighters.moveset.ron", 0x52ec27c93c5c7836),
    ("template/assets/content/grunt.action.ron", 0x2f57162bf76b869d),
    ("template/assets/content/hero.action.ron", 0xb7618b94af666dd1),
    ("template/assets/content/movesets.moveset.ron", 0x09f1e4aae7e05354),
    ("template/assets/content/tuning.combat_tuning.ron", 0x0cf2e43e38f71b04),
];

fn hash_of(path: &str) -> u64 {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let text = std::fs::read_to_string(format!("{root}/{path}")).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut kinds = ContentKinds::default();
    kinds.register::<ActionDef>();
    kinds.register::<MovesetDef>();
    kinds.register::<StatusDef>();
    kinds.register::<CombatTuning>();
    ContentDb::build(&kinds, &[Source { path: path.into(), text }]).hash()
}

#[test]
fn new_fields_keep_old_json() {
    let mut now = Vec::new();
    for (path, _) in GOLDEN {
        now.push((*path, hash_of(path)));
    }
    if std::env::var("PRINT_GOLDEN").is_ok() {
        for (path, hash) in &now {
            println!("    (\"{path}\", {hash:#018x}),");
        }
    }
    for ((path, want), (_, got)) in GOLDEN.iter().zip(&now) {
        assert_eq!(got, want, "{path}: its definitions no longer normalize to the JSON they had at 988723b");
    }
}
