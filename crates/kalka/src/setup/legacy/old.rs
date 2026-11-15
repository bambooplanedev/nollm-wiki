//! What `wiki` 0.6.13 wrote, as the takeover recognises it: the values, the digests of its texts,
//! and the marks of its entries in a file.

use crate::hash;
use serde_json::{json, Value};

pub(super) const OLD_SKILL: &str = ".agents/skills/wiki-navigation/SKILL.md";
pub(super) const OLD_HOOK: &str = ".pi/extensions/wiki-hint/index.ts";
pub(super) const NAV_BEGIN: &str = "<!-- wiki:navigation -->\n";
pub(super) const NAV_END: &str = "<!-- /wiki:navigation -->\n";

// BLAKE3 (`crate::hash`) hex digests of every text `wiki` 0.6.13 wrote or took for its own, built
// the way its `setup/mod.rs` built them from the files of `v0.6.13:wiki/src/setup/`.

/// The pi skill: `SKILL.md` with `<memory-dir>` set to `.agents/memory` (mod.rs:317), and each
/// `shipped/skill-N.md` that way or cut before `\n# Memory\n` (mod.rs:322-333). The variant with
/// the per-user memory folder (mod.rs:328) holds a machine's path, so it cannot be listed here: a
/// skill holding it is reported, not removed. Variants equal to another one are listed once.
pub(super) const OLD_SKILL_DIGESTS: [&str; 11] = [
    "4317b9f0e72faaf2312662142a4d8c89a11d5d3669667e86c9cdad7dcead4953", // SKILL.md
    "b79037b38ed68393d12982cdbfdd66b14139a5136ac0d84cd8fca5f9720b52c9", // shipped/skill-1.md
    "28a164ccfae1f03d8a463d69b6f99c32640b70443570b85b5ff3a83d5a1d3543", // shipped/skill-2.md
    "ef03b30699c42cdd8fecfd8d0cb63cff1e721a86b55a2ea2cf9fd72d9daf0684", // shipped/skill-3.md
    "89ea720825f8fc32fa560a5e4ad3d60d0f79aca3a00dc8e47dec2a89dce99c06", // shipped/skill-4.md
    "4477eb068724310792501f487eb8a9ca01c52fb688a11a4c8e40213b8f08660c", // shipped/skill-5.md
    "5ed51420afb208078e7cd61762a908d9e8d112c6a76d52bae73265c4dfb37399", // shipped/skill-5.md, cut
    "db639e69510644935275f4bc9711db487f43a9df22a81f29f6e8fae55a02e981", // shipped/skill-6.md
    "70c3991d123f699290547e6e6f02befba73a5a5a418fa3cca3cbd3a21dfdf7dd", // shipped/skill-6.md, cut
    "a56b3152987b58e2e6e825c4e4d181da9314ff9bbb52e53b8ba6d54e2686a79f", // shipped/skill-7.md
    "449507b06bfb0e3fd37c6c7aa9d387e0d7c80ebf15fe1f028ad1906b902ff44e", // shipped/skill-8.md
];

/// The pi extension: `<cmd>` set to `wiki` (mod.rs:529-533).
pub(super) const OLD_HOOK_DIGESTS: [&str; 2] = [
    "b9d49d3d23a6a0c1ebb7cfe4e85fd22fe2622d6b25fed285538afc2a1bee4a07", // wiki-hint.ts
    "9c2e8a391c38f3420a7e2ddf1089858d9263238664b0663da79a5354f72700e9", // shipped/wiki-hint-1.ts
];

/// The `AGENTS.md` block: `<!-- wiki:navigation -->\n`, the text, `<!-- /wiki:navigation -->\n`
/// (mod.rs:341-344, 422), the span the old `remove_block` looked for (edit.rs:261-263).
pub(super) const OLD_BLOCK_DIGESTS: [&str; 3] = [
    "eadaad9c02e99861d27fd581a47a5739284d80cf830e426fc7ee85fab583f5c9", // navigation.md
    "74018e9acb5790edf573193892e6816c6853226eeb4e5f1d9f470ba85201ec76", // shipped/navigation-1.md
    "ac98c83130b3969252b397ab361edad3ebcf6b3d6f98a7cd944eb2c533a865a0", // shipped/navigation-2.md
];

/// The server entry, in every file and for Claude Code's CLI (mod.rs:388).
pub(super) fn old_entry() -> Value {
    json!({"command": "wiki", "args": ["serve", "--source", "."]})
}

pub(super) fn known(text: &str, digests: &[&str]) -> bool {
    digests.contains(&hash::to_hex(&hash::hash_str(text)).as_str())
}

/// From the first begin line through the end line after it: the bytes the digests cover.
pub(super) fn block(text: &str) -> Option<&str> {
    let i = text.find(NAV_BEGIN)?;
    let j = text[i..].find(NAV_END)?;
    Some(&text[i..i + j + NAV_END.len()])
}

/// Whether text (`None`: there, but not readable) holds the old tool's server `wiki`.
pub(super) fn old_server(t: Option<&str>) -> bool {
    let v = t.and_then(|t| serde_json::from_str::<Value>(t).ok());
    v.as_ref()
        .and_then(|v| v.get("mcpServers")?.as_object())
        .is_some_and(|m| m.contains_key("wiki"))
}

/// Whether a settings text holds a prompt hook item running `wiki hint`.
pub(super) fn old_hook(t: Option<&str>) -> bool {
    let Some(t) = t else { return false };
    let Ok(v) = serde_json::from_str::<Value>(t) else {
        return false;
    };
    let items = v
        .pointer("/hooks/UserPromptSubmit")
        .and_then(Value::as_array);
    items
        .into_iter()
        .flatten()
        .filter_map(|i| i["hooks"].as_array())
        .flatten()
        .any(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.starts_with("wiki hint"))
        })
}

pub(super) fn old_block(t: Option<&str>) -> bool {
    t.is_some_and(|t| t.lines().any(|l| l == NAV_BEGIN.trim_end()))
}
