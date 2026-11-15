use super::toml::*;
use crate::setup::{entry, CMD, LAUNCH};
use serde_json::Value;

const B: &str =
    "[mcp_servers.calque]\ncommand = \"calque\"\nargs = [\"serve\", \"--source\", \".\"]\n";
const BN: &str = "[mcp_servers.calque]\ncommand = \"calque\"\nargs = [\"serve\", \"--source\", \".\", \"--memory\", \"none\"]\n";

/// calque's own entries by name, `memory`'s variant first.
fn own(memory: bool) -> [Value; 2] {
    [entry(CMD, memory), entry(CMD, !memory)]
}

fn crlf(s: &str) -> String {
    s.replace('\n', "\r\n")
}

#[test]
fn the_block_is_the_exact_text() {
    assert_eq!(block(&entry(CMD, true), "\n"), B);
    assert_eq!(block(&entry(CMD, false), "\n"), BN);
}

#[test]
fn exact_blocks_are_ours_wherever_they_sit() {
    assert_eq!(state(B, &own(true)), State::Ours);
    assert_eq!(state(BN, &own(false)), State::Ours);
    assert_eq!(state(B, &own(false)), State::OursOther);
    assert_eq!(state(BN, &own(true)), State::OursOther);
    assert_eq!(state(&crlf(B), &own(true)), State::Ours);
    assert_eq!(state(B.trim_end(), &own(true)), State::Ours);
    let spaced = "  [mcp_servers.calque]  \n\ncommand = \"calque\"  \nargs = [\"serve\", \"--source\", \".\"] \n\n";
    assert_eq!(state(spaced, &own(true)), State::Ours);
    let around = format!("# mine\n[a]\nx = 1\n\n{B}\n[b]\ny = 2\n");
    assert_eq!(state(&around, &own(true)), State::Ours);
    assert_eq!(
        state(&format!("[a]\n[[b]]\nz = 1\n\n{B}"), &own(true)),
        State::Ours
    );
}

#[test]
fn nothing_of_calque_is_absent() {
    for t in [
        "",
        "\n",
        "model = \"o3\"\n",
        "# [mcp_servers.calque]\n",
        "[mcp_servers]\nother = { command = \"x\" }\ncalquex = { command = \"y\" }\n",
        "[mcp_servers.other]\ncommand = \"calque\"\nargs = [\"calque\"]\n",
        "[mcp_servers.calquex]\ncommand = \"calque\"\n",
    ] {
        assert_eq!(state(t, &own(true)), State::Absent, "{t:?}");
    }
}

const PROXY: &str = "[mcp_servers.proxy]\ncommand = \"mcp-proxy\"\nargs = [\n  \"--name\",\n  \"calque\",\n  \"calque\"\n]\n";

#[test]
fn a_word_calque_that_is_no_key_is_no_server() {
    let prose = "[mcp_servers.docs]\ncommand = \"docs\"\n\n[profiles.x]\ndeveloper_instructions = \"\"\"\ncalque: use the wiki first\n\"\"\"\n";
    for t in [PROXY, prose] {
        assert_eq!(state(t, &own(true)), State::Absent, "{t:?}");
        assert_eq!(
            state(&t.replace('\n', "\r\n"), &own(true)),
            State::Absent,
            "{t:?}"
        );
    }
    assert_eq!(
        with_block(PROXY, &entry(CMD, true)),
        "[mcp_servers.proxy]\ncommand = \"mcp-proxy\"\nargs = [\n  \"--name\",\n  \"calque\",\n  \"calque\"\n]\n\n[mcp_servers.calque]\ncommand = \"calque\"\nargs = [\"serve\", \"--source\", \".\"]\n"
    );
    // A key named calque after any `[mcp_servers…]` header is still refused, however spelled.
    for key in [
        "calque = { command = \"y\" }",
        "calque.command = \"y\"",
        "\"calque\" = { command = \"y\" }",
        "'calque'.command = \"y\"",
        "calque   = { command = \"y\" }",
        "calque .command = \"y\"",
    ] {
        let t = format!("{PROXY}[mcp_servers]\n{key}\n");
        assert_eq!(state(&t, &own(true)), State::Other(ANOTHER), "{t:?}");
    }
}

#[test]
fn any_other_calque_definition_is_another_server() {
    let body = "command = \"calque\"\nargs = [\"serve\", \"--source\", \".\"]\n";
    for t in [
        format!("{B}env = {{ A = \"1\" }}\n"),
        format!("{B}cwd = \"x\"\n"),
        format!("{B}enabled = false\n"),
        format!("[mcp_servers.calque]\n# mine\n{body}"),
        format!("[mcp_servers.calque] # mine\n{body}"),
        format!("[mcp_servers.calque]\n  {body}"),
        "[mcp_servers.calque]\ncommand = \"calque\"\nargs = [\"serve\",\"--source\",\".\"]\n"
            .into(),
        "[mcp_servers.calque]\ncommand = \"other\"\nargs = [\"serve\", \"--source\", \".\"]\n"
            .into(),
        "[mcp_servers.calque]\n".into(),
        format!("[mcp_servers.calque]\n[x]\n{body}"),
        format!("{B}\n{B}"),
        format!("[mcp_servers.\"calque\"]\n{body}"),
        format!("[mcp_servers.'calque']\n{body}"),
        format!("[ mcp_servers . calque ]\n{body}"),
        format!("[\"mcp_servers\".calque]\n{body}"),
        format!("{B}[mcp_servers.calque.env]\nA = \"1\"\n"),
        "[mcp_servers]\ncalque.command = \"calque\"\n".into(),
        "[mcp_servers]\ncalque = { command = \"calque\" }\n".into(),
        "[mcp_servers]\n\"calque\" = { command = \"calque\" }\n".into(),
        "[ mcp_servers ] # all\ncalque .command = \"calque\"\n".into(),
        "mcp_servers.calque.command = \"calque\"\n".into(),
        "mcp_servers = { calque = { command = \"calque\" } }\n".into(),
        format!("mcp_servers = {{ calque = {{}} }}\n{B}"),
    ] {
        assert_eq!(state(&t, &own(true)), State::Other(ANOTHER), "{t:?}");
    }
}

#[test]
fn a_line_that_only_looks_like_a_header_hides_nothing() {
    // A `[` line inside a multi-line string or array is no table: what follows is still read.
    let doc = "doc = \"\"\"\n[docs](https://example.com)\n\"\"\"\n";
    let notify = "notify = [\n  [\"a\"],\n]\n";
    for (t, why) in [
        (
            format!("{doc}mcp_servers = {{ other = {{ command = \"x\" }} }}\n"),
            UNEXTENDABLE,
        ),
        (
            format!("{doc}mcp_servers.calque.command = \"y\"\n"),
            ANOTHER,
        ),
        (
            format!("{notify}mcp_servers = {{ calque = {{ command = \"y\" }} }}\n"),
            ANOTHER,
        ),
        (format!("{notify}mcp_servers = {{}}\n"), UNEXTENDABLE),
        (
            "[mcp_servers]\nx = \"\"\"\n[x]\n\"\"\"\ncalque = { command = \"y\" }\n".to_string(),
            ANOTHER,
        ),
        (
            "[profiles.x]\nmcp_servers.calque.command = \"x\"\n".into(),
            ANOTHER,
        ),
        ("[profiles.x]\nmcp_servers = {}\n".into(), UNEXTENDABLE),
    ] {
        assert_eq!(state(&t, &own(true)), State::Other(why), "{t:?}");
    }
    // A byte order mark: refused, never stripped (the header behind it would not be found).
    for t in [
        "\u{feff}[mcp_servers]\ncalque = { command = \"y\" }\n".to_string(),
        format!("\u{feff}{B}"),
        "\u{feff}model = \"o3\"\n".into(),
    ] {
        assert_eq!(state(&t, &own(true)), State::Other(BOM), "{t:?}");
    }
}

#[test]
fn an_mcp_servers_table_calque_cannot_extend_is_not_written_to() {
    for t in [
        "mcp_servers = {}\n",
        "mcp_servers = { other = { command = \"x\" } }\n",
        "mcp_servers.other.command = \"x\"\n",
        "[[mcp_servers]]\nname = \"x\"\n",
        "[[mcp_servers.other]]\nname = \"x\"\n",
    ] {
        assert_eq!(state(t, &own(true)), State::Other(UNEXTENDABLE), "{t:?}");
    }
}

#[test]
fn the_block_goes_at_the_end_after_one_blank_line() {
    assert_eq!(with_block("", &entry(CMD, true)), B);
    assert_eq!(with_block("", &entry(CMD, false)), BN);
    assert_eq!(
        with_block("model = \"o3\"\n", &entry(CMD, true)),
        format!("model = \"o3\"\n\n{B}")
    );
    assert_eq!(
        with_block("model = \"o3\"", &entry(CMD, true)),
        format!("model = \"o3\"\n\n{B}")
    );
    assert_eq!(
        with_block("[x]\ny = 1\n# end\n", &entry(CMD, true)),
        format!("[x]\ny = 1\n# end\n\n{B}")
    );
    assert_eq!(
        with_block("a = 1\r\n", &entry(CMD, true)),
        format!("a = 1\r\n\r\n{}", crlf(B))
    );
    assert_eq!(
        with_block("a = 1\r\nb = 2", &entry(CMD, true)),
        format!("a = 1\r\nb = 2\r\n\r\n{}", crlf(B))
    );
    // Mixed: the ending of the last line that has one.
    assert_eq!(
        with_block("a = 1\r\nb = 2\n", &entry(CMD, true)),
        format!("a = 1\r\nb = 2\n\n{B}")
    );
    assert_eq!(
        with_block("a = 1\nb = 2\r\nc = 3", &entry(CMD, true)),
        format!("a = 1\nb = 2\r\nc = 3\r\n\r\n{}", crlf(B))
    );
}

#[test]
fn a_switch_rewrites_only_the_command_and_args_lines() {
    assert_eq!(with_body(B, &entry(CMD, false)), BN);
    assert_eq!(with_body(BN, &entry(CMD, true)), B);
    assert_eq!(with_body(&crlf(B), &entry(CMD, false)), crlf(BN));
    assert_eq!(with_body(B.trim_end(), &entry(CMD, false)), BN.trim_end());
    let around = |b: &str| format!("# top\r\n[a]\nx = 1\n\n{b}\n[b]\ny = 2\n");
    assert_eq!(with_body(&around(B), &entry(CMD, false)), around(BN));
    let spaced =
        "[mcp_servers.calque]\n\ncommand = \"calque\"\nargs = [\"serve\", \"--source\", \".\"]  \n";
    assert_eq!(
        with_body(spaced, &entry(CMD, false)),
        "[mcp_servers.calque]\n\ncommand = \"calque\"\nargs = [\"serve\", \"--source\", \".\", \"--memory\", \"none\"]\n"
    );
    // From the kulman home, the block that starts calque by name is calque's own and rewritten.
    let launch = [entry(LAUNCH, true), entry(CMD, true)];
    assert_eq!(state(&around(&crlf(B)), &launch), State::OursOther);
    assert_eq!(
        with_body(&around(&crlf(B)), &launch[0]),
        around(&crlf(
            "[mcp_servers.calque]\ncommand = \"sh\"\nargs = [\"-c\", \"exec \\\"${KULMAN_HOME:-$HOME/.kulman-home}/bin/calque\\\" serve --source .\"]\n"
        ))
    );
}

#[test]
fn removal_leaves_everything_else_byte_for_byte() {
    assert_eq!(without_block(B), "");
    assert_eq!(without_block(BN.trim_end()), "");
    assert_eq!(without_block(&format!("a = 1\n\n{B}")), "a = 1\n");
    assert_eq!(
        without_block(&format!("[a]\nx = 1\n\n{B}\n[b]\ny = 2\n")),
        "[a]\nx = 1\n\n[b]\ny = 2\n"
    );
    assert_eq!(
        without_block(&format!("# top\n\n{B}# after\n")),
        "# top\n# after\n"
    );
    assert_eq!(
        without_block(&format!("a = 1\r\n\r\n{}", crlf(B))),
        "a = 1\r\n"
    );
    // Only one blank line before it is the block's.
    assert_eq!(without_block(&format!("a = 1\n\n\n{B}")), "a = 1\n\n");
    for t in [
        "",
        "model = \"o3\"\n",
        "[x]\ny = 1\n\n\n",
        "a = 1\r\nb = 2\r\n",
        "[a]\r\n[b]\n",
    ] {
        for m in [true, false] {
            assert_eq!(without_block(&with_block(t, &entry(CMD, m))), t, "{t:?}");
        }
    }
}
