use super::*;

const B: &str =
    "[mcp_servers.kalka]\ncommand = \"kalka\"\nargs = [\"serve\", \"--source\", \".\"]\n";
const BN: &str = "[mcp_servers.kalka]\ncommand = \"kalka\"\nargs = [\"serve\", \"--source\", \".\", \"--memory\", \"none\"]\n";

fn crlf(s: &str) -> String {
    s.replace('\n', "\r\n")
}

#[test]
fn the_block_is_the_exact_text() {
    assert_eq!(block(true, "\n"), B);
    assert_eq!(block(false, "\n"), BN);
}

#[test]
fn exact_blocks_are_ours_wherever_they_sit() {
    assert_eq!(state(B, true), State::Ours);
    assert_eq!(state(BN, false), State::Ours);
    assert_eq!(state(B, false), State::OursOtherArgs);
    assert_eq!(state(BN, true), State::OursOtherArgs);
    assert_eq!(state(&crlf(B), true), State::Ours);
    assert_eq!(state(B.trim_end(), true), State::Ours);
    let spaced = "  [mcp_servers.kalka]  \n\ncommand = \"kalka\"  \nargs = [\"serve\", \"--source\", \".\"] \n\n";
    assert_eq!(state(spaced, true), State::Ours);
    let around = format!("# mine\n[a]\nx = 1\n\n{B}\n[b]\ny = 2\n");
    assert_eq!(state(&around, true), State::Ours);
    assert_eq!(
        state(&format!("[a]\n[[b]]\nz = 1\n\n{B}"), true),
        State::Ours
    );
}

#[test]
fn nothing_of_kalka_is_absent() {
    for t in [
        "",
        "\n",
        "model = \"o3\"\n",
        "# [mcp_servers.kalka]\n",
        "[mcp_servers]\nother = { command = \"x\" }\nkalkax = { command = \"y\" }\n",
        "[mcp_servers.other]\ncommand = \"kalka\"\nargs = [\"kalka\"]\n",
        "[mcp_servers.kalkax]\ncommand = \"kalka\"\n",
        "[profiles.x]\nmcp_servers.kalka.command = \"x\"\n",
    ] {
        assert_eq!(state(t, true), State::Absent, "{t:?}");
    }
}

#[test]
fn any_other_kalka_definition_is_another_server() {
    let body = "command = \"kalka\"\nargs = [\"serve\", \"--source\", \".\"]\n";
    for t in [
        format!("{B}env = {{ A = \"1\" }}\n"),
        format!("{B}cwd = \"x\"\n"),
        format!("{B}enabled = false\n"),
        format!("[mcp_servers.kalka]\n# mine\n{body}"),
        format!("[mcp_servers.kalka] # mine\n{body}"),
        format!("[mcp_servers.kalka]\n  {body}"),
        "[mcp_servers.kalka]\ncommand = \"kalka\"\nargs = [\"serve\",\"--source\",\".\"]\n".into(),
        "[mcp_servers.kalka]\ncommand = \"other\"\nargs = [\"serve\", \"--source\", \".\"]\n"
            .into(),
        "[mcp_servers.kalka]\n".into(),
        format!("[mcp_servers.kalka]\n[x]\n{body}"),
        format!("{B}\n{B}"),
        format!("[mcp_servers.\"kalka\"]\n{body}"),
        format!("[mcp_servers.'kalka']\n{body}"),
        format!("[ mcp_servers . kalka ]\n{body}"),
        format!("[\"mcp_servers\".kalka]\n{body}"),
        format!("\u{feff}[mcp_servers.kalka]\n{body}"),
        format!("{B}[mcp_servers.kalka.env]\nA = \"1\"\n"),
        "[mcp_servers]\nkalka.command = \"kalka\"\n".into(),
        "[mcp_servers]\nkalka = { command = \"kalka\" }\n".into(),
        "[mcp_servers]\n\"kalka\" = { command = \"kalka\" }\n".into(),
        "[ mcp_servers ] # all\nkalka .command = \"kalka\"\n".into(),
        "mcp_servers.kalka.command = \"kalka\"\n".into(),
        "mcp_servers = { kalka = { command = \"kalka\" } }\n".into(),
        format!("mcp_servers = {{ kalka = {{}} }}\n{B}"),
    ] {
        assert_eq!(state(&t, true), State::Other(ANOTHER), "{t:?}");
    }
}

#[test]
fn an_mcp_servers_table_kalka_cannot_extend_is_not_written_to() {
    for t in [
        "mcp_servers = {}\n",
        "mcp_servers = { other = { command = \"x\" } }\n",
        "mcp_servers.other.command = \"x\"\n",
        "[[mcp_servers]]\nname = \"x\"\n",
        "[[mcp_servers.other]]\nname = \"x\"\n",
    ] {
        assert_eq!(state(t, true), State::Other(UNEXTENDABLE), "{t:?}");
    }
}

#[test]
fn the_block_goes_at_the_end_after_one_blank_line() {
    assert_eq!(with_block("", true), B);
    assert_eq!(with_block("", false), BN);
    assert_eq!(
        with_block("model = \"o3\"\n", true),
        format!("model = \"o3\"\n\n{B}")
    );
    assert_eq!(
        with_block("model = \"o3\"", true),
        format!("model = \"o3\"\n\n{B}")
    );
    assert_eq!(
        with_block("[x]\ny = 1\n# end\n", true),
        format!("[x]\ny = 1\n# end\n\n{B}")
    );
    assert_eq!(
        with_block("a = 1\r\n", true),
        format!("a = 1\r\n\r\n{}", crlf(B))
    );
    assert_eq!(
        with_block("a = 1\r\nb = 2", true),
        format!("a = 1\r\nb = 2\r\n\r\n{}", crlf(B))
    );
    // Mixed: the ending of the last line that has one.
    assert_eq!(
        with_block("a = 1\r\nb = 2\n", true),
        format!("a = 1\r\nb = 2\n\n{B}")
    );
    assert_eq!(
        with_block("a = 1\nb = 2\r\nc = 3", true),
        format!("a = 1\nb = 2\r\nc = 3\r\n\r\n{}", crlf(B))
    );
}

#[test]
fn a_switch_rewrites_only_the_args_line() {
    assert_eq!(with_args(B, false), BN);
    assert_eq!(with_args(BN, true), B);
    assert_eq!(with_args(&crlf(B), false), crlf(BN));
    assert_eq!(with_args(B.trim_end(), false), BN.trim_end());
    let around = |b: &str| format!("# top\r\n[a]\nx = 1\n\n{b}\n[b]\ny = 2\n");
    assert_eq!(with_args(&around(B), false), around(BN));
    let spaced =
        "[mcp_servers.kalka]\n\ncommand = \"kalka\"\nargs = [\"serve\", \"--source\", \".\"]  \n";
    assert_eq!(
        with_args(spaced, false),
        "[mcp_servers.kalka]\n\ncommand = \"kalka\"\nargs = [\"serve\", \"--source\", \".\", \"--memory\", \"none\"]\n"
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
            assert_eq!(without_block(&with_block(t, m)), t, "{t:?}");
        }
    }
}
