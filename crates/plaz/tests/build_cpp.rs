mod common;

use common::headings;

const KILN_H: &str = "class Kiln {\npublic:\n    Kiln();\n    void fire(int t);\n    void vent();\n    int heat() const {\n        if (hot_) { return 9; }\n        return 0;\n    }\nprivate:\n    bool hot_;\n};\n";
const KILN_CPP: &str = "#include \"kiln.h\"\nstatic int budget = start_budget();\nKiln::Kiln() : hot_(false) {}\nvoid Kiln::fire(int t) {\n    if (t == 3) { hot_ = true; }\n}\n";

#[test]
fn c_and_cpp_functions_are_charted_in_sources_and_headers() {
    let (_d, vault) = common::built(&[("src/kiln.h", KILN_H), ("src/kiln.cpp", KILN_CPP)]);
    let cpp = common::map_page(&vault, "map/src/kiln_cpp");
    assert!(!cpp.contains("Not covered yet"), "{cpp}");
    assert_eq!(headings(&cpp), ["kiln_kiln", "kiln_fire"]);
    let fire = common::fn_block(&cpp, "kiln_fire");
    assert!(fire.contains("    n1{\"t == 3\"}\n"), "{fire}");
    assert!(
        fire.contains(
            "\nCoverage: 0 calls: 0 resolved, 0 one of, 0 outside, 0 hidden. Not drawn: none.\n"
        ),
        "{fire}"
    );
    let h = common::map_page(&vault, "map/src/kiln_h");
    let heat = common::fn_block(&h, "kiln_heat");
    assert!(heat.contains("    n1{\"hot_\"}\n"), "{heat}");
}

#[test]
fn a_declaration_links_its_definition_or_says_there_is_none() {
    let (_d, vault) = common::built(&[("src/kiln.h", KILN_H), ("src/kiln.cpp", KILN_CPP)]);
    let h = common::map_page(&vault, "map/src/kiln_h");
    assert_eq!(
        headings(&h),
        ["kiln_kiln", "kiln_fire", "kiln_vent", "kiln_heat"]
    );
    assert_eq!(
        common::fn_block(&h, "kiln_fire"),
        "## kiln_fire\n`void fire(int t)` · `src/kiln.h:4-4`\n\nDefined at [[map/src/kiln_cpp#kiln_fire|Kiln::fire]]\n\n"
    );
    assert_eq!(
        common::fn_block(&h, "kiln_vent"),
        "## kiln_vent\n`void vent()` · `src/kiln.h:5-5`\n\nNo definition in the project.\n\n"
    );
    let cpp = common::map_page(&vault, "map/src/kiln_cpp");
    assert!(cpp.contains("\n## kiln_fire\n"), "{cpp}");
}

#[test]
fn a_constructor_says_constructions_are_not_traced_and_file_level_calls_are_noted() {
    let (_d, vault) = common::built(&[("src/kiln.h", KILN_H), ("src/kiln.cpp", KILN_CPP)]);
    let cpp = common::map_page(&vault, "map/src/kiln_cpp");
    assert_eq!(
        common::fn_section(&cpp, "kiln_kiln", "Called from"),
        ["constructions and operator uses are not traced"]
    );
    assert_eq!(
        common::fn_section(&cpp, "kiln_fire", "Called from"),
        ["none found"]
    );
    let module = "\nModule-level code is not charted.\n";
    assert!(cpp.contains(module), "{cpp}");
    assert!(!common::map_page(&vault, "map/src/kiln_h").contains(module));
}

#[test]
fn a_prototype_links_its_definition_in_the_same_file() {
    let calc = "static int clamp(int v);\nint run(int v) { return clamp(v); }\nstatic int clamp(int v) { return v > 9 ? 9 : v; }\n";
    // A same-named function elsewhere is not the prototype's definition.
    let other = "static int clamp(int v) { return v; }\n";
    let (_d, vault) = common::built(&[("src/calc.c", calc), ("src/other.c", other)]);
    let page = common::map_page(&vault, "map/src/calc_c");
    assert_eq!(headings(&page), ["clamp", "run", "clamp_2"]);
    assert_eq!(
        common::fn_block(&page, "clamp"),
        "## clamp\n`static int clamp(int v)` · `src/calc.c:1-1`\n\nDefined at [[map/src/calc_c#clamp_2|clamp]]\n\n"
    );
}

#[test]
fn same_arity_overloads_the_index_cannot_pair_are_one_of_their_definitions() {
    let h = "class Pan {\npublic:\n    void heat(int t);\n    void heat(double t);\n};\n";
    let cpp = "#include \"pan.h\"\nvoid Pan::heat(int t) { warm(t); }\nvoid Pan::heat(double t) { warm(t); }\n";
    let (_d, vault) = common::built(&[("src/pan.h", h), ("src/pan.cpp", cpp)]);
    let header = common::map_page(&vault, "map/src/pan_h");
    let one_of = "Defined at one of: [[map/src/pan_cpp#pan_heat|Pan::heat]], [[map/src/pan_cpp#pan_heat_2|Pan::heat]]";
    for (anchor, sig, line) in [
        ("pan_heat", "void heat(int t)", 3),
        ("pan_heat_2", "void heat(double t)", 4),
    ] {
        assert_eq!(
            common::fn_block(&header, anchor).trim_end(),
            format!("## {anchor}\n`{sig}` · `src/pan.h:{line}-{line}`\n\n{one_of}")
        );
    }
    assert_eq!(
        headings(&common::map_page(&vault, "map/src/pan_cpp")),
        ["pan_heat", "pan_heat_2"]
    );
}

#[test]
fn a_declaration_is_defined_only_by_a_namesake_of_its_own_namespace() {
    // `rise` has one namesake the index pairs it with, `fall` two.
    let tide = "namespace tide { void rise(); void fall(); void ebb(); }\n";
    let wave =
        "namespace wave { void rise() {} void fall() {} }\nnamespace tide { void ebb() {} }\n";
    let gust = "namespace gust { void fall() {} }\n";
    let files = [
        ("src/tide.h", tide),
        ("src/wave.cpp", wave),
        ("src/gust.cpp", gust),
    ];
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, "map/src/tide_h");
    for name in ["rise", "fall"] {
        assert_eq!(
            common::fn_block(&page, name).trim_end(),
            format!(
                "## {name}\n`void {name}()` · `src/tide.h:1-1`\n\nNo definition in the project."
            )
        );
    }
    assert_eq!(
        common::fn_block(&page, "ebb").trim_end(),
        "## ebb\n`void ebb()` · `src/tide.h:1-1`\n\nDefined at [[map/src/wave_cpp#ebb|ebb]]"
    );
}

#[test]
fn a_methods_declaration_is_defined_only_by_a_method_of_its_own_class() {
    let h = "namespace one { struct Gear { void spin(int v); }; }\nnamespace two { struct Gear { void spin(int v); }; }\nstruct Gear { void spin(int v); };\n";
    let def = "#include \"gear.h\"\nvoid one::Gear::spin(int v) {}\nnamespace two { void Gear::spin(int v) {} }\nvoid Gear::spin(int v) {}\n";
    let (_d, vault) = common::built(&[("src/gear.h", h), ("src/gear.cpp", def)]);
    let page = common::map_page(&vault, "map/src/gear_h");
    for (anchor, line, def) in [
        ("gear_spin", 1, "gear_spin"),
        ("gear_spin_2", 2, "gear_spin_2"),
        ("gear_spin_3", 3, "gear_spin_3"),
    ] {
        assert_eq!(
            common::fn_block(&page, anchor).trim_end(),
            format!("## {anchor}\n`void spin(int v)` · `src/gear.h:{line}-{line}`\n\nDefined at [[map/src/gear_cpp#{def}|Gear::spin]]")
        );
    }
}

#[test]
fn a_methods_declaration_keeps_a_definition_whose_class_no_head_places() {
    // `Gear` at file scope may be either class: as when a parse error
    // loses the namespace around the definition.
    let h = "namespace one { struct Gear { void spin(int v); }; }\nnamespace two { struct Gear { void spin(int v); }; }\n";
    let def = "#include \"gear.h\"\nvoid Gear::spin(int v) {}\n";
    let (_d, vault) = common::built(&[("src/gear.h", h), ("src/gear.cpp", def)]);
    let page = common::map_page(&vault, "map/src/gear_h");
    for (anchor, line) in [("gear_spin", 1), ("gear_spin_2", 2)] {
        assert_eq!(
            common::fn_block(&page, anchor).trim_end(),
            format!("## {anchor}\n`void spin(int v)` · `src/gear.h:{line}-{line}`\n\nDefined at [[map/src/gear_cpp#gear_spin|Gear::spin]]")
        );
    }
}

#[test]
fn a_function_both_trees_list_is_one_call_target_with_a_heading() {
    let src = "#ifdef WIDE\nlong span(long a) {\n#else\nint span(int a) {\n#endif\n    return grow(a);\n}\nint use_it() { return span(1); }\n";
    let (_d, vault) = common::built(&[("a.cpp", src)]);
    let page = common::map_page(&vault, "map/a_cpp");
    assert_eq!(headings(&page), ["span", "use_it"]);
    assert_eq!(
        common::fn_section(&page, "use_it", "Calls"),
        ["`span(1)` L8 → [[map/a_cpp#span|span]]"]
    );
}

#[test]
fn a_defaulted_or_deleted_function_says_so_and_has_no_chart() {
    let h = "class Pot {\npublic:\n    Pot() = default;\n    Pot(const Pot&) = delete;\n    int size() const { return 1; }\n};\n";
    let (_d, vault) = common::built(&[("src/pot.h", h)]);
    let page = common::map_page(&vault, "map/src/pot_h");
    assert_eq!(headings(&page), ["pot_pot", "pot_pot_2", "pot_size"]);
    assert_eq!(
        common::fn_block(&page, "pot_pot"),
        "## pot_pot\n`Pot() = default` · `src/pot.h:3-3`\n\nDefaulted.\n\n"
    );
    assert_eq!(
        common::fn_block(&page, "pot_pot_2"),
        "## pot_pot_2\n`Pot(const Pot&) = delete` · `src/pot.h:4-4`\n\nDeleted.\n\n"
    );
}

#[test]
fn a_call_of_a_jumping_macro_from_another_file_is_on_the_coverage_line() {
    let guard = "#define RETURN_UNLESS(c) do { if (!(c)) return; } while (0)\n";
    let src = "void feed(int* p, int a) {\n    RETURN_UNLESS(a > 0);\n    use(p);\n}\n";
    let (_d, vault) = common::built(&[("inc/guard.h", guard), ("src/a.cpp", src)]);
    let feed = common::fn_block(&common::map_page(&vault, "map/src/a_cpp"), "feed");
    let coverage: Vec<&str> = feed
        .lines()
        .filter(|l| l.starts_with("Coverage: "))
        .collect();
    assert_eq!(
        coverage,
        ["Coverage: 2 calls: 0 resolved, 0 one of, 1 outside, 1 hidden. Not drawn: 1 macro jump."]
    );
}

/// Each call of the charted function `name` in `path`, in source order,
/// with its target: `path:line` of the target item, `one of: …`, `outside`
/// or `hidden`.
fn cpp_targets(files: &[(&str, &str)], path: &str, name: &str) -> Vec<String> {
    use plaz::model::Target;
    let sources = files.iter().map(|(p, t)| (p.to_string(), t.to_string()));
    let resolved = plaz::resolve::resolve(plaz::source::project(sources));
    let p = resolved.project();
    let at = |id: &plaz::model::FnId| {
        let line = p.files[&id.path].code.items[id.item].start_line;
        format!("{}:{line}", id.path)
    };
    let f = &p.files[path];
    let (_, flow) = (f.flows.iter())
        .find(|(&i, _)| f.code.items[i].name == name)
        .unwrap_or_else(|| panic!("no charted {name} in {path}"));
    let mut out = Vec::new();
    flow.each_call(&mut |_, c| {
        let target = match &c.target {
            Target::Resolved(id) => at(id),
            Target::OneOf {
                candidates,
                outside,
            } => {
                let all: Vec<String> = candidates.iter().map(at).collect();
                let or = if *outside { ", or outside" } else { "" };
                format!("one of: {}{or}", all.join(", "))
            }
            Target::Outside => "outside".to_string(),
            Target::Hidden => "hidden".to_string(),
        };
        out.push(format!("{} → {target}", c.text));
    });
    out
}

#[test]
fn a_call_of_a_file_scope_variable_in_sight_is_not_visible() {
    let h = "extern void (*on_halt)(int);\n";
    let src = "#include \"hooks.h\"\n#include <stdio.h>\ntypedef int (*order_t)(int, int);\nstatic void quiet(const char* m) {}\nstatic void (*say)(const char*) = quiet;\norder_t rank;\nvoid speak(const char* m) {\n    say(m);\n    rank(1, 2);\n    on_halt(0);\n    quiet(m);\n    puts(m);\n}\n";
    assert_eq!(
        cpp_targets(&[("src/hooks.h", h), ("src/a.c", src)], "src/a.c", "speak"),
        [
            "say(m) → hidden",
            "rank(1, 2) → hidden",
            "on_halt(0) → hidden",
            "quiet(m) → src/a.c:4",
            "puts(m) → outside"
        ]
    );
}

#[test]
fn a_cpp_call_of_a_bound_name_is_hidden_beside_a_same_named_function() {
    let src = "int pick(int v) { return v; }\nint run(int (*pick)(int)) { return pick(2); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", src)], "src/a.c", "run"),
        ["pick(2) → hidden"]
    );
}

#[test]
fn a_c_definition_in_the_callers_file_wins_over_an_included_declaration() {
    // Valid C: one function, its header declaration spelled differently.
    let h = "int count(char *argv[]);\n";
    let src = "#include \"count.h\"\nint count(char **argv) { return argv != 0; }\nint run(char **v) { return count(v); }\n";
    assert_eq!(
        cpp_targets(&[("src/count.h", h), ("src/a.c", src)], "src/a.c", "run"),
        ["count(v) → src/a.c:2"]
    );
}

#[test]
fn a_method_declared_unlike_its_files_one_definition_may_be_another_overload() {
    let src = "struct Pot {\n    void put(int v);\n    void put(double v);\n    void run();\n};\nvoid Pot::put(double v) {}\nvoid Pot::run() { put(1); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["put(1) → one of: src/a.cpp:6, or outside"]
    );
}

#[test]
fn a_cpp_callers_own_overload_and_an_included_one_are_one_candidate_set() {
    let h = "int step(double d);\n";
    let src = "#include \"step.h\"\nstatic int step(int a) { return a; }\nint run() { return step(1.5); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/step.h", h), ("src/step.cpp", src)],
            "src/step.cpp",
            "run"
        ),
        ["step(1.5) → one of: src/step.cpp:2, src/step.h:1"]
    );
}

#[test]
fn a_forward_declaration_folds_onto_its_definition_in_the_same_file() {
    // REPORT T12, first bullet.
    let src = "static int twice(int);\nint run(int a) { return twice(a); }\nstatic int twice(int a) { return a * 2; }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", src)], "src/a.c", "run"),
        ["twice(a) → src/a.c:3"]
    );
}

#[test]
fn a_header_declaration_folds_onto_its_definition_with_names_and_defaults_dropped() {
    let h = "int scale(const struct grid* g, const std::vector<enum mode>& ms, int value, int by = 2);\n";
    let def =
        "#include \"scale.h\"\nint scale(const grid *g, const std::vector<mode> &m, const int v, int k) { return v * k; }\n";
    let main = "#include \"scale.h\"\nint run() { return scale(nullptr, {}, 3); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("src/scale.h", h),
                ("src/scale.cpp", def),
                ("src/main.cpp", main)
            ],
            "src/main.cpp",
            "run"
        ),
        ["scale(nullptr, {}, 3) → src/scale.cpp:2"]
    );
}

#[test]
fn a_declaration_folds_onto_a_definition_spelled_with_top_level_const_or_another_qualifier() {
    let h = "namespace core {\nenum class Kind { A };\nstruct Tally {\n    void add(core::Kind k, int n = 1);\n    void mark(int* const at);\n    void tag(const core::Kind& k);\n};\nvoid loose(Kind k, int n = 1);\n}\n";
    let def = "#include \"tally.h\"\nusing core::Kind;\nnamespace core {\nvoid Tally::add(const Kind k, const int n) {}\nvoid Tally::mark(int* at) {}\nvoid Tally::tag(const Kind& k) {}\n}\nvoid core::loose(const core::Kind k, int n) {}\n";
    let main = "#include \"tally.h\"\nvoid run(core::Tally& t, core::Kind k) { t.add(k); t.mark(nullptr); t.tag(k); core::loose(k); }\n";
    let files = [
        ("src/tally.h", h),
        ("src/tally.cpp", def),
        ("src/main.cpp", main),
    ];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        [
            "t.add(k) → src/tally.cpp:4",
            "t.mark(nullptr) → src/tally.cpp:5",
            "t.tag(k) → src/tally.cpp:6",
            "core::loose(k) → src/tally.cpp:8"
        ]
    );
}

#[test]
fn a_declaration_does_not_fold_onto_a_definition_of_other_parameter_types() {
    // `w` reads `Pad` unqualified in two definitions: either may be it;
    // `v`'s `Pad` may name another class than `two::Pad`.
    let h = "namespace one { struct Pad {}; }\nnamespace two { struct Pad {}; }\nstruct Pad {};\nvoid put(int n);\nvoid f(const char* s);\nvoid g(const Pad& p);\nvoid q(one::Pad p);\nvoid w(Pad p);\nvoid v(Pad p);\n";
    let def = "#include \"put.h\"\nvoid put(double d) {}\nvoid f(char* s) {}\nvoid g(Pad& p) {}\nvoid q(two::Pad p) {}\nvoid w(one::Pad p) {}\nvoid w(two::Pad p) {}\nvoid v(two::Pad p) {}\n";
    let main = "#include \"put.h\"\nvoid run(Pad p, one::Pad o) { put(1); f(\"x\"); g(p); q(o); w(p); v(p); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("src/put.h", h),
                ("src/put.cpp", def),
                ("src/main.cpp", main)
            ],
            "src/main.cpp",
            "run"
        ),
        [
            "put(1) → src/put.h:4",
            "f(\"x\") → src/put.h:5",
            "g(p) → src/put.h:6",
            "q(o) → src/put.h:7",
            "w(p) → src/put.h:8",
            "v(p) → src/put.h:9"
        ]
    );
}

#[test]
fn a_declaration_folds_across_a_qualifier_only_where_its_file_shows_the_short_name_is_the_long_one()
{
    // `put`'s `text` is the library's; `log`'s is `my::text`, by a `using`
    // in an included header; `cut`'s `wide_t` is a global type of the
    // library's header, nothing showing it to be `my::wide_t`.
    let h = "#include <text>\n#include \"my.h\"\nusing lib::text;\nvoid put(const text& s);\nvoid cut(wide_t w);\n";
    let my = "namespace my { struct text {}; struct wide_t {}; }\n";
    let log = "#include <text>\n#include \"my.h\"\n#include \"use.h\"\nvoid log(const text& s);\n";
    let def = "#include \"put.h\"\n#include \"log.h\"\nvoid put(const my::text& s) {}\nvoid log(const my::text& s) {}\nvoid cut(my::wide_t w) {}\n";
    let main = "#include \"put.h\"\n#include \"log.h\"\nvoid run(const my::text& t, wide_t w) { put(\"x\"); log(t); cut(w); }\n";
    let files = [
        ("src/put.h", h),
        ("src/my.h", my),
        ("src/use.h", "using namespace my;\n"),
        ("src/log.h", log),
        ("src/put.cpp", def),
        ("src/main.cpp", main),
    ];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        [
            "put(\"x\") → src/put.h:4",
            "log(t) → src/put.cpp:4",
            "cut(w) → src/put.h:5"
        ]
    );
}

#[test]
fn a_declaration_folds_across_an_alias_of_its_namespace_or_its_classs_own_name() {
    // `path` is the alias around `shift`, naming what `fs::path` names;
    // `Crate::Crate::Lid` is `Crate::Lid`.
    let h = "namespace io {\nusing path = lib::fs::path;\nint shift(const path& p);\n}\nstruct Crate {\n    struct Lid {};\n};\nint seal(const Crate::Lid& l);\n";
    let def = "#include \"io.h\"\nnamespace io {\nnamespace fs = lib::fs;\nint shift(const fs::path& p) { return 1; }\n}\nint seal(const Crate::Crate::Lid& l) { return 2; }\n";
    let main = "#include \"io.h\"\nint run(const io::path& p, const Crate::Lid& l) { return io::shift(p) + seal(l); }\n";
    let files = [("src/io.h", h), ("src/io.cpp", def), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        ["io::shift(p) → src/io.cpp:4", "seal(l) → src/io.cpp:6"]
    );
}

#[test]
fn a_declaration_does_not_fold_across_a_nested_class_of_the_same_name() {
    let h = "namespace a { struct Kind {}; }\nstruct Box {\n    struct Kind {};\n    void f(Kind k);\n    void f(a::Kind k);\n};\n";
    let def = "#include \"box.h\"\nvoid Box::f(a::Kind k) {}\n";
    let main = "#include \"box.h\"\nvoid run(Box& b, Box::Kind k) { b.f(k); }\n";
    let files = [
        ("src/box.h", h),
        ("src/box.cpp", def),
        ("src/main.cpp", main),
    ];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        ["b.f(k) → one of: src/box.cpp:2, src/box.h:4"]
    );
}

#[test]
fn a_function_in_the_include_closure_is_certain_when_one_and_one_of_when_several() {
    let top = "#include \"../lib/mid.h\"\n";
    let mid = "#include \"deep.h\"\nint pair_of(int a);\n";
    let deep = "int sink(int a);\nint pair_of(long a);\n";
    // The definitions, in a file no include reaches; it includes the
    // declaring header, so they are the declaration's.
    let elsewhere = "#include \"../lib/deep.h\"\nint sink(int a) { return a; }\nint sink(long a) { return 1; }\n";
    let src = "#include \"top.h\"\nint run() { return sink(1) + pair_of(2); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("app/top.h", top),
                ("lib/mid.h", mid),
                ("lib/deep.h", deep),
                ("other/sink.cpp", elsewhere),
                // Same suffix as the include, not the file it names.
                ("vendor/lib/mid.h", "int sink(short a);\n"),
                ("app/main.cpp", src),
            ],
            "app/main.cpp",
            "run"
        ),
        [
            "sink(1) → other/sink.cpp:2",
            "pair_of(2) → one of: lib/deep.h:2, lib/mid.h:2"
        ]
    );
}

#[test]
fn a_static_inline_helper_in_a_header_is_certain_only_where_the_header_is_included() {
    let h = "static inline int clampi(int v) { return v < 0 ? 0 : v; }\n";
    let with = "#include \"util.h\"\nint run(int v) { return clampi(v); }\n";
    let without = "int other(int v) { return clampi(v); }\n";
    let files = [
        ("src/util.h", h),
        ("src/with.c", with),
        ("src/without.c", without),
    ];
    assert_eq!(
        cpp_targets(&files, "src/with.c", "run"),
        ["clampi(v) → src/util.h:1"]
    );
    assert_eq!(
        cpp_targets(&files, "src/without.c", "other"),
        ["clampi(v) → outside"]
    );
}

#[test]
fn a_project_function_no_include_reaches_may_be_a_c_library_name() {
    // REPORT T13.
    let net = "#include <unistd.h>\nvoid hang_up(int fd) { close(fd); }\n";
    let pool = "void close(int slot) {}\n";
    assert_eq!(
        cpp_targets(
            &[("src/net.cpp", net), ("src/pool.cpp", pool)],
            "src/net.cpp",
            "hang_up"
        ),
        ["close(fd) → one of: src/pool.cpp:1, or outside"]
    );
}

#[test]
fn a_static_function_of_another_file_is_no_candidate() {
    // REPORT T12, second bullet.
    let a = "static int twice(int a) { return a * 2; }\n";
    let b = "int run() { return twice(1); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", a), ("src/b.c", b)], "src/b.c", "run"),
        ["twice(1) → outside"]
    );
}

#[test]
fn a_function_in_an_anonymous_namespace_of_another_file_is_no_candidate() {
    let a = "namespace {\nint twice(int a) { return a * 2; }\n}\nint keep() { return twice(2); }\n";
    let b = "int run() { return twice(1); }\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "keep"),
        ["twice(2) → src/a.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "run"),
        ["twice(1) → outside"]
    );
}

#[test]
fn a_variadic_function_is_a_candidate_for_any_argument_count() {
    // REPORT T12, third bullet.
    let src = "int say(const char* f, ...) { return 0; }\nint run() { return say(\"a\", 1, 2); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.c", src)], "src/a.c", "run"),
        ["say(\"a\", 1, 2) → src/a.c:1"]
    );
}

#[test]
fn a_bare_call_of_a_project_macro_or_type_is_hidden_anything_else_outside() {
    let h = "#define CHECKED(x) (x)\nstruct Point { int x; };\n";
    let src = "#include \"defs.h\"\nint run() { CHECKED(1); Point(2); return puts(\"x\"); }\n";
    assert_eq!(
        cpp_targets(&[("inc/defs.h", h), ("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "CHECKED(1) → hidden",
            "Point(2) → hidden",
            "puts(\"x\") → outside"
        ]
    );
}

#[test]
fn a_function_a_project_macro_generates_is_outside() {
    // A known limitation pinned on purpose: no item stands for `get_depth`.
    let src = "#define DEFINE_GETTER(x) int get_##x() { return 0; }\nDEFINE_GETTER(depth)\nint run() { return get_depth(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["get_depth() → outside"]
    );
}

#[test]
fn a_std_call_is_outside_and_a_root_the_project_does_not_have_is_outside() {
    let src = "int run() { std::abs(1); return fmtlib::print(2); }\n";
    // A same-named project function changes neither, nor a project
    // specialization in `namespace std`.
    let other = "int print(int a) { return a; }\nint abs(int a) { return a; }\nnamespace std {\ntemplate <> struct hash<int>;\n}\n";
    assert_eq!(
        cpp_targets(
            &[("src/a.cpp", src), ("src/print.cpp", other)],
            "src/a.cpp",
            "run"
        ),
        ["std::abs(1) → outside", "fmtlib::print(2) → outside"]
    );
}

#[test]
fn a_call_qualified_by_a_project_type_is_its_member_not_dispatched() {
    let h = "struct Base {\n    virtual void fill(int n);\n};\nstruct Child : Base {\n    void fill(int n) override { Base::fill(n); }\n};\n";
    let cpp = "#include \"base.h\"\nvoid Base::fill(int k) {}\n";
    assert_eq!(
        cpp_targets(
            &[("src/base.h", h), ("src/base.cpp", cpp)],
            "src/base.h",
            "fill"
        ),
        ["Base::fill(n) → src/base.cpp:2"]
    );
}

#[test]
fn a_call_qualified_by_a_project_namespace_reaches_its_free_function() {
    let h = "namespace core {\nint run(int n);\n}\n";
    let def = "#include \"core.h\"\nint core::run(int count) { return count; }\n";
    let main = "#include \"core.h\"\nint start() { return core::run(1); }\n";
    assert_eq!(
        cpp_targets(
            &[
                ("src/core.h", h),
                ("src/core.cpp", def),
                ("src/main.cpp", main)
            ],
            "src/main.cpp",
            "start"
        ),
        ["core::run(1) → src/core.cpp:2"]
    );
}

#[test]
fn a_call_qualified_from_the_global_namespace_reaches_its_free_function() {
    let h = "namespace io {\nint open(int fd);\n}\n";
    let def = "#include \"io.h\"\nint io::open(int fd) { return fd; }\n";
    let main = "#include \"io.h\"\nint start() { return ::io::open(1) + ::std::abs(2); }\n";
    let files = [("src/io.h", h), ("src/io.cpp", def), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "start"),
        ["::io::open(1) → src/io.cpp:2", "::std::abs(2) → outside"]
    );
}

#[test]
fn a_call_written_from_the_global_namespace_skips_the_callers_namespaces() {
    let h = "namespace net { namespace port { int open(int fd); } }\nint open(int fd);\nnamespace io { int open(int fd); }\nnamespace detail { int helper(int v); }\nnamespace b { namespace detail { int helper(int v); } }\nstruct Gate { static int make(); };\nnamespace rig { struct Gate { static int make(); }; struct Lone { static int make(); }; }\n";
    let main = "#include \"x.h\"\nnamespace net { int a() { return ::port::open(1); } }\nnamespace io { int c() { return ::open(2); } }\nnamespace b { int d() { return ::detail::helper(3); } }\nnamespace rig { int g() { return ::Gate::make() + ::Lone::make(); } }\nint e() { auto open = [](int v) { return v; }; return ::open(4); }\n";
    // A file-scope `using` in an included header may bring the name there.
    let used = "#include \"x.h\"\n#include \"u.h\"\nint f() { return ::port::open(5) + ::Lone::make(); }\n";
    let files = [
        ("src/x.h", h),
        ("src/main.cpp", main),
        ("src/u.h", "using namespace net;\nusing rig::Lone;\n"),
        ("src/used.cpp", used),
    ];
    let calls: Vec<String> = ["a", "c", "d", "g", "e"]
        .iter()
        .flat_map(|f| cpp_targets(&files, "src/main.cpp", f))
        .collect();
    assert_eq!(
        calls,
        [
            "::port::open(1) → outside",
            "::open(2) → src/x.h:2",
            "::detail::helper(3) → src/x.h:4",
            "::Gate::make() → src/x.h:6",
            "::Lone::make() → outside",
            "::open(4) → src/x.h:2"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/used.cpp", "f"),
        [
            "::port::open(5) → hidden",
            "::Lone::make() → one of: src/x.h:7, or outside"
        ]
    );
}

#[test]
fn a_qualifier_longer_than_the_calls_shown_text_is_read_whole() {
    let src = "struct Crate {\n    static int make(int v) { return v; }\n    template <class A, class B, class C> struct Tray {\n        static int make(int v) { return v + 1; }\n    };\n};\nint run() {\n    return Crate::Tray<unsigned long long, unsigned long long, unsigned long long, unsigned long long>::make(1);\n}\nint cut() { return Crate::Tray<int, int, decltype(run())>::make(2) + Crate::Tray<int, int, sizeof(int)>::make(3); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["Crate::Tray<unsigned long long, unsigned long long, unsigned long long, unsigne… → src/a.cpp:4"]
    );
    // A parenthesis in the template arguments does not end the qualifier.
    let mut cut = cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "cut");
    cut.retain(|c| c.starts_with("Crate"));
    assert_eq!(
        cut,
        [
            "Crate::Tray<int, int, decltype(run())>::make(2) → src/a.cpp:4",
            "Crate::Tray<int, int, sizeof(int)>::make(3) → src/a.cpp:4"
        ]
    );
}

#[test]
fn a_declaration_does_not_fold_onto_a_namesake_in_another_outer_namespace() {
    let h = "namespace snow { namespace util {\nint melt(int v);\n} }\n";
    let def = "#include \"snow.h\"\nnamespace rain { namespace util {\nint melt(int v) { return v; }\n} }\n";
    let main = "#include \"snow.h\"\nint thaw() { return snow::util::melt(2); }\nnamespace snow {\nint drip() { return util::melt(1); }\n}\n";
    let files = [
        ("src/snow.h", h),
        ("src/rain.cpp", def),
        ("src/main.cpp", main),
    ];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "thaw"),
        ["snow::util::melt(2) → src/snow.h:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "drip"),
        ["util::melt(1) → src/snow.h:2"]
    );
}

#[test]
fn a_method_of_a_class_named_like_its_namespace_folds_onto_its_definition() {
    let h = "namespace kiosk {\nstruct kiosk {\n    void open(int n);\n};\n}\n";
    let def = "#include \"kiosk.h\"\nnamespace kiosk {\nvoid kiosk::open(int n) {}\n}\n";
    let main = "#include \"kiosk.h\"\nvoid start(kiosk::kiosk& k) { k.open(1); }\n";
    let files = [
        ("src/kiosk.h", h),
        ("src/kiosk.cpp", def),
        ("src/main.cpp", main),
    ];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "start"),
        ["k.open(1) → src/kiosk.cpp:3"]
    );
}

#[test]
fn a_qualified_call_reaches_only_functions_of_the_namespace_path_it_writes() {
    let snow = "namespace snow { namespace util {\nint melt(int v);\n} }\n";
    let rain = "namespace rain { namespace util {\nint melt(int v);\n} }\n";
    let def = "#include \"rain.h\"\nnamespace rain { namespace util {\nint melt(int v) { return v; }\n} }\n";
    // A header relying on its includer for `snow.h`.
    let side = "#include \"rain.h\"\ninline int puddle() { return snow::util::melt(3); }\n";
    let files = [
        ("src/snow.h", snow),
        ("src/rain.h", rain),
        ("src/rain.cpp", def),
        ("src/side.h", side),
    ];
    assert_eq!(
        cpp_targets(&files, "src/side.h", "puddle"),
        ["snow::util::melt(3) → one of: src/snow.h:2, or outside"]
    );
}

#[test]
fn a_prototype_offers_no_static_definition_of_another_file() {
    let proto = "int measure(int v);\nint run(int v) { return measure(v); }\n";
    let shared = "int measure(int v) { return v; }\n";
    let hidden = "static int measure(int v) { return -v; }\n";
    let (_d, vault) = common::built(&[
        ("src/proto.c", proto),
        ("src/shared.c", shared),
        ("src/hidden.c", hidden),
    ]);
    assert_eq!(
        common::fn_block(&common::map_page(&vault, "map/src/proto_c"), "measure"),
        "## measure\n`int measure(int v)` · `src/proto.c:1-1`\n\nDefined at [[map/src/shared_c#measure|measure]]\n\n"
    );
}

#[test]
fn a_headers_call_of_a_static_inline_in_a_header_it_does_not_include_may_be_it() {
    let b = "static inline int halve(int v) { return v / 2; }\n";
    let a = "static inline int quarter(int v) { return halve(halve(v)); }\n";
    let src = "int run(int v) { return halve(v); }\n";
    let files = [("inc/b.h", b), ("inc/a.h", a), ("src/run.c", src)];
    assert_eq!(
        cpp_targets(&files, "inc/a.h", "quarter"),
        [
            "halve(v) → one of: inc/b.h:1, or outside",
            "halve(halve(v)) → one of: inc/b.h:1, or outside"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/run.c", "run"),
        ["halve(v) → outside"]
    );
}

#[test]
fn a_call_qualified_by_a_template_parameter_is_any_project_method_of_that_name() {
    let h = "struct Cell {\n    static int make(int n);\n};\n";
    let src = "#include \"cell.h\"\ntemplate <typename T>\nint build(int n) { return T::make(n) + T::drop(n) + Cell::make(n) + std::abs(n); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/cell.h", h), ("src/build.cpp", src)],
            "src/build.cpp",
            "build"
        ),
        [
            "T::make(n) → one of: src/cell.h:2, or outside",
            "T::drop(n) → outside",
            "Cell::make(n) → src/cell.h:2",
            "std::abs(n) → outside"
        ]
    );
}

#[test]
fn a_call_qualified_by_a_block_scope_alias_is_any_project_method_of_that_name() {
    let h = "template <typename T>\nstruct Box {\n    static T fill(T v);\n};\n";
    let src = "#include \"box.h\"\nint run() {\n    using X = Box<int>;\n    return X::fill(1) + Box<int>::fill(2);\n}\n";
    assert_eq!(
        cpp_targets(
            &[("src/box.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "run"
        ),
        [
            "X::fill(1) → one of: src/box.h:3, or outside",
            "Box<int>::fill(2) → src/box.h:3"
        ]
    );
}

#[test]
fn a_file_scope_using_directive_names_its_namespace_exactly_an_inner_one_by_suffix() {
    let h = "namespace io {\nint put(int v);\n}\nnamespace net { namespace io {\nint put(int v);\n} }\n";
    let top = "#include \"io.h\"\nusing namespace io;\nint run() { return put(1); }\n";
    let inner =
        "#include \"io.h\"\nnamespace net {\nusing namespace io;\nint go() { return put(2); }\n}\n";
    let files = [("src/io.h", h), ("src/a.cpp", top), ("src/b.cpp", inner)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["put(1) → src/io.h:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "go"),
        ["put(2) → one of: src/io.h:2, src/io.h:5"]
    );
}

#[test]
fn a_bare_call_reaches_a_namespace_function_only_from_that_namespace_or_a_using() {
    let h = "namespace io {\nint close(int fd);\nstruct Pipe { void drain(int fd); };\n}\n";
    let shut = "#include <unistd.h>\n#include \"fdutil.h\"\nvoid shut(int fd) { close(fd); }\nnamespace io {\nvoid again(int fd) { close(fd); }\n}\nvoid io::Pipe::drain(int fd) { close(fd); }\n";
    let used = "#include \"fdutil.h\"\nusing namespace io;\nvoid done(int fd) { close(fd); }\n";
    let files = [
        ("src/fdutil.h", h),
        ("src/shut.cpp", shut),
        ("src/used.cpp", used),
    ];
    let decl = "close(fd) → src/fdutil.h:2";
    assert_eq!(
        cpp_targets(&files, "src/shut.cpp", "shut"),
        ["close(fd) → one of: src/fdutil.h:2, or outside"]
    );
    assert_eq!(cpp_targets(&files, "src/shut.cpp", "again"), [decl]);
    assert_eq!(cpp_targets(&files, "src/shut.cpp", "drain"), [decl]);
    assert_eq!(cpp_targets(&files, "src/used.cpp", "done"), [decl]);
}

#[test]
fn an_angle_include_with_a_directory_reaches_a_project_header() {
    let h = "static inline int clampi(int v) { return v < 0 ? 0 : v; }\n";
    let with = "#include <proj/util.h>\nint run(int v) { return clampi(v); }\n";
    let bare = "#include <util.h>\nint other(int v) { return clampi(v); }\n";
    let files = [
        ("include/proj/util.h", h),
        ("src/angle.c", with),
        ("src/bare.c", bare),
    ];
    assert_eq!(
        cpp_targets(&files, "src/angle.c", "run"),
        ["clampi(v) → include/proj/util.h:1"]
    );
    assert_eq!(
        cpp_targets(&files, "src/bare.c", "other"),
        ["clampi(v) → outside"]
    );
}

#[test]
fn an_inline_file_included_at_file_scope_is_read_and_one_included_in_a_body_is_not() {
    // `gauge.inl` stands on its own; `rows.inl` is a piece of `Grid`'s
    // body, where `fill` is a method, and `spare.ipp` is included nowhere.
    let h = "namespace kit {\nstruct Gauge {\n    template <class T> int read(T v);\n};\nint trim(int v);\n}\n#include \"gauge.inl\"\n";
    let inl = "namespace kit {\ntemplate <class T> int Gauge::read(T v) { return trim(1); }\ninline int trim(int v) { return v; }\n}\n";
    let grid = "#include \"gauge.h\"\nstruct Grid {\n#include \"rows.inl\"\n};\nint fill(int v);\nint run(kit::Gauge& g) { return g.read(2) + fill(3) + kit::trim(4); }\n";
    let files = [
        ("src/gauge.h", h),
        ("src/gauge.inl", inl),
        ("src/rows.inl", "int fill(int v) { return v; }\n"),
        ("src/spare.ipp", "int fill(int v) { return v + 1; }\n"),
        ("src/grid.cpp", grid),
    ];
    assert_eq!(
        cpp_targets(&files, "src/grid.cpp", "run"),
        [
            "g.read(2) → src/gauge.inl:2",
            "fill(3) → src/grid.cpp:5",
            "kit::trim(4) → src/gauge.inl:3"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/gauge.inl", "read"),
        ["trim(1) → src/gauge.inl:3"]
    );
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, "map/src/gauge_h");
    assert_eq!(
        common::fn_block(&page, "trim"),
        "## trim\n`int trim(int v)` · `src/gauge.h:5-5`\n\nDefined at [[map/src/gauge_inl#trim|trim]]\n"
    );
    assert!(!vault.join("map/src/rows_inl.md").exists());
    assert!(!vault.join("map/src/spare_ipp.md").exists());
}

#[test]
fn a_forward_declaration_spelled_otherwise_folds_onto_the_files_one_definition() {
    let c = "static int count(char *argv[]);\nint run(char **v) { return count(v); }\nstatic int count(char **argv) { return argv != 0; }\n";
    // C++: the definition of the same arity, not the overload; it may
    // be another overload, defined elsewhere.
    let cpp = "int tally(int v[]);\nint run(int *v) { return tally(v); }\nint tally(int *v) { return *v; }\nint tally() { return 0; }\n";
    assert_eq!(
        cpp_targets(&[("src/fwd.c", c)], "src/fwd.c", "run"),
        ["count(v) → src/fwd.c:3"]
    );
    assert_eq!(
        cpp_targets(&[("src/fwd.cpp", cpp)], "src/fwd.cpp", "run"),
        ["tally(v) → one of: src/fwd.cpp:3, or outside"]
    );
}

#[test]
fn a_namespace_named_like_inline_keeps_its_name() {
    let h = "namespace inline_math {\nint sq(int v);\n}\n";
    let src = "#include \"m.h\"\nint run() { return inline_math::sq(2); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/m.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "run"
        ),
        ["inline_math::sq(2) → src/m.h:2"]
    );
}

#[test]
fn a_function_of_an_inline_namespace_is_its_own_and_its_parents_by_name() {
    // `h` is defined with the inline namespace written, `k` without it.
    let h = "inline namespace v1 { int h(int v); }\nnamespace outer { inline namespace v2 { int k(int v); } }\n";
    let def =
        "#include \"n.h\"\nint v1::h(int v) { return v; }\nint outer::k(int v) { return v; }\n";
    let main =
        "#include \"n.h\"\nusing namespace outer::v2;\nint run() { return h(1) + v1::h(2) + outer::k(3) + outer::v2::k(4) + k(5); }\n";
    let files = [("src/n.h", h), ("src/n.cpp", def), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        [
            "h(1) → src/n.cpp:2",
            "v1::h(2) → src/n.cpp:2",
            "outer::k(3) → src/n.cpp:3",
            "outer::v2::k(4) → src/n.cpp:3",
            "k(5) → src/n.cpp:3"
        ]
    );
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, "map/src/n_h");
    assert_eq!(
        common::fn_block(&page, "h"),
        "## h\n`int h(int v)` · `src/n.h:1-1`\n\nDefined at [[map/src/n_cpp#v1_h|v1::h]]\n\n"
    );
    assert_eq!(
        common::fn_block(&page, "k"),
        "## k\n`int k(int v)` · `src/n.h:2-2`\n\nDefined at [[map/src/n_cpp#outer_k|outer::k]]\n"
    );
}

#[test]
fn two_inline_namespaces_of_one_parent_keep_their_functions_apart() {
    let h = "namespace rim {\ninline namespace v1 { int spoke(int v); int turn(int v); int hub(int v); }\ninline namespace v2 { int spoke(int v); int hub(int v); }\n}\n";
    // `hub` is defined in the parent, where either namespace's may be.
    let def = "#include \"rim.h\"\nnamespace rim { inline namespace v2 { int spoke(int v) { return v; } } }\nnamespace rim { inline namespace v1 { int turn(int v) { return spoke(v); } } }\nint rim::hub(int v) { return v; }\n";
    let main = "#include \"rim.h\"\nint run() { return rim::v1::spoke(1) + rim::v2::spoke(2) + rim::spoke(3) + rim::v1::hub(4); }\n";
    // Another library's plain namespace of an inline one's name.
    let other = "namespace v1 { int h(int v) { return v; } }\nint h(int v) { return v + 1; }\nint both() { return v1::h(1) + h(2); }\n";
    let files = [
        ("src/rim.h", h),
        ("src/rim.cpp", def),
        ("src/main.cpp", main),
        ("lib/n.h", "inline namespace v1 { int h(int v); }\n"),
        ("other/o.cpp", other),
    ];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        [
            "rim::v1::spoke(1) → src/rim.h:2",
            "rim::v2::spoke(2) → src/rim.cpp:2",
            "rim::spoke(3) → one of: src/rim.cpp:2, src/rim.h:2",
            "rim::v1::hub(4) → src/rim.h:2"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/rim.cpp", "turn"),
        ["spoke(v) → one of: src/rim.cpp:2, src/rim.h:2"]
    );
    assert_eq!(
        cpp_targets(&files, "other/o.cpp", "both"),
        ["v1::h(1) → other/o.cpp:1", "h(2) → other/o.cpp:2"]
    );
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, "map/src/rim_h");
    assert_eq!(
        common::fn_block(&page, "spoke"),
        "## spoke\n`int spoke(int v)` · `src/rim.h:2-2`\n\nNo definition in the project.\n\n"
    );
    assert_eq!(
        common::fn_block(&page, "hub"),
        "## hub\n`int hub(int v)` · `src/rim.h:2-2`\n\nNo definition in the project.\n\n"
    );
    assert_eq!(
        common::fn_block(&page, "spoke_2"),
        "## spoke_2\n`int spoke(int v)` · `src/rim.h:3-3`\n\nDefined at [[map/src/rim_cpp#spoke|spoke]]\n"
    );
}

#[test]
fn an_out_of_line_conversion_operator_calls_its_classs_members() {
    let h = "namespace rig {\nstruct Gate {\n    int charge;\n    bool valid(int c) const;\n    operator int() const;\n};\n}\n";
    let src = "#include \"gate.h\"\nbool rig::Gate::valid(int c) const { return c > 0; }\nrig::Gate::operator int() const { return valid(charge); }\n";
    let files = [("src/gate.h", h), ("src/gate.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/gate.cpp", "operator int"),
        ["valid(charge) → src/gate.cpp:2"]
    );
}

#[test]
fn a_cpp_lambda_capturing_by_default_calls_a_local_of_the_enclosing_function_unseen() {
    let src = "int pick() { return 0; }\nint handler(int v) { return v; }\nvoid run() {\n    auto handler = pick();\n    auto go = [&] { handler(1); };\n    go();\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "pick() → src/a.cpp:1",
            "handler(1) → hidden",
            "go() → hidden"
        ]
    );
}

#[test]
fn a_cpp_method_reaches_members_free_functions_and_typed_receivers() {
    // REPORT T11, with names of its own.
    let h = "#pragma once\nnamespace acct { namespace ledger {\nint tally(int x);\nint pick(int a);\nint pick(double a);\nint pick(int a, int b);\nclass Ledger {\npublic:\n    virtual void post(int q);\n    void close_day(int q, int (*hook)(int));\n    void settle(int n);\nprivate:\n    void recount_();\n};\nstruct Branch : public Ledger {\n    void post(int q) override;\n};\n}}\n";
    let src = "#include \"ledger.h\"\n#include \"util/note.h\"\n#include <fmtx/core.h>\n#include <algorithm>\nnamespace acct { namespace ledger {\nvoid Ledger::recount_() {}\nvoid Ledger::post(int q) {}\nvoid Branch::post(int q) { Ledger::post(q); }\nint tally(int x) { return x; }\nint pick(int a) { return a; }\nint pick(double a) { return 0; }\nint pick(int a, int b) { return a + b; }\nvoid Ledger::settle(int n) {}\nvoid Ledger::close_day(int q, int (*hook)(int)) {\n    auto dbl = [](int v) { return v; };\n    Ledger& other = *this;\n    Branch c;\n    recount_();\n    this->recount_();\n    post(q);\n    dbl(3);\n    hook(7);\n    tally(q);\n    util::note(\"day\");\n    acct::ledger::tally(1);\n    std::sort(nullptr, nullptr);\n    fmtx::print(\"x\");\n    other.settle(1);\n    c.post(1);\n    pick(1);\n}\n}}\n";
    let files = [
        ("src/ledger.h", h),
        ("src/ledger.cpp", src),
        (
            "src/util/note.h",
            "namespace util {\nvoid note(const char* m);\n}\n",
        ),
        (
            "src/util/note.cpp",
            "#include \"note.h\"\nnamespace util {\nvoid note(const char* m) {}\n}\n",
        ),
    ];
    assert_eq!(
        cpp_targets(&files, "src/ledger.cpp", "close_day"),
        [
            "recount_() → src/ledger.cpp:6",
            "this->recount_() → src/ledger.cpp:6",
            "post(q) → one of: src/ledger.cpp:7, src/ledger.cpp:8",
            "dbl(3) → hidden",
            "hook(7) → hidden",
            "tally(q) → src/ledger.cpp:9",
            "util::note(\"day\") → src/util/note.cpp:3",
            "acct::ledger::tally(1) → src/ledger.cpp:9",
            "std::sort(nullptr, nullptr) → outside",
            "fmtx::print(\"x\") → outside",
            "other.settle(1) → src/ledger.cpp:13",
            "c.post(1) → src/ledger.cpp:8",
            "pick(1) → one of: src/ledger.cpp:10, src/ledger.cpp:11, src/ledger.cpp:12",
        ]
    );
}

#[test]
fn a_bare_call_in_a_method_reaches_the_nearest_base_that_defines_it() {
    let src = "struct Top {\n    void lift() {}\n    void drop() {}\n};\nstruct Mid : Top {\n    void lift() {}\n};\nstruct Low : public Mid {\n    void run() { lift(); drop(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["lift() → src/a.cpp:6", "drop() → src/a.cpp:3"]
    );
}

#[test]
fn a_member_a_subclass_of_the_base_also_defines_is_one_of_both() {
    let src = "struct Top {\n    virtual void lift() {}\n};\nstruct Mid : Top {\n    void run() { lift(); }\n};\nstruct Low : Mid {\n    void lift() override {}\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["lift() → one of: src/a.cpp:2, src/a.cpp:8"]
    );
}

#[test]
fn a_class_with_an_outside_base_calls_a_bare_name_as_maybe_inherited() {
    let h = "int flush(int n);\n";
    let src = "#include \"flush.h\"\nstruct Sink : public lib::Stream<int> {\n    void run() { flush(1); sync(); }\n};\n";
    assert_eq!(
        cpp_targets(
            &[("src/flush.h", h), ("src/sink.cpp", src)],
            "src/sink.cpp",
            "run"
        ),
        [
            "flush(1) → one of: src/flush.h:1, or outside",
            "sync() → outside"
        ]
    );
}

#[test]
fn a_base_outside_the_project_counts_through_a_project_base() {
    let h = "int flush(int n);\n";
    let src = "#include \"flush.h\"\nstruct Mid : std::streambuf {};\nstruct Sink : Mid {\n    void run() { flush(1); }\n};\n";
    assert_eq!(
        cpp_targets(
            &[("src/flush.h", h), ("src/sink.cpp", src)],
            "src/sink.cpp",
            "run"
        ),
        ["flush(1) → one of: src/flush.h:1, or outside"]
    );
}

#[test]
fn this_arrow_a_name_the_class_lacks_is_outside_under_an_outside_base() {
    let src = "struct Pane : ui::Widget {\n    void draw() { this->paint(); }\n};\nstruct Ink {\n    void paint() {}\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "draw"),
        ["this->paint() → outside"]
    );
}

#[test]
fn this_arrow_a_name_the_class_lacks_under_project_bases_is_any_project_method() {
    let src = "struct Card {\n    void show() { this->paint(); }\n};\nstruct Ink {\n    void paint() {}\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "show"),
        ["this->paint() → one of: src/a.cpp:5, or outside"]
    );
}

#[test]
fn a_namespace_owner_is_no_class_even_when_a_type_shares_its_name() {
    let h = "namespace text {\nstruct text {\n    void trim();\n};\nint lines();\n}\n";
    let src = "#include \"text.h\"\nint text::lines() { trim(); return 0; }\n";
    assert_eq!(
        cpp_targets(
            &[("src/text.h", h), ("src/text.cpp", src)],
            "src/text.cpp",
            "lines"
        ),
        ["trim() → outside"]
    );
}

#[test]
fn a_macro_before_a_return_type_naming_a_project_class_makes_no_member_call() {
    let h = "struct Store {\n    void stock();\n    struct Shelf {};\n};\n";
    let src = "#include \"shelf.h\"\nAPI Store::Shelf restock() { stock(); return {}; }\n";
    assert_eq!(
        cpp_targets(
            &[("src/shelf.h", h), ("src/shelf.cpp", src)],
            "src/shelf.cpp",
            "restock"
        ),
        ["stock() → outside"]
    );
}

#[test]
fn a_smart_pointer_receiver_is_outside_with_a_dot_and_its_pointee_with_an_arrow() {
    let src = "struct Book {\n    void apply() {}\n    void reset() {}\n};\nvoid run() {\n    auto p = std::make_unique<Book>();\n    p.reset();\n    p->apply();\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "std::make_unique<Book>() → outside",
            "p.reset() → outside",
            "p->apply() → src/a.cpp:2"
        ]
    );
}

#[test]
fn a_typed_receiver_of_a_class_another_class_overrides_is_one_of_both() {
    let src = "struct Shape {\n    virtual int area() { return 0; }\n};\nstruct Square : Shape {\n    int area() override { return 1; }\n};\nint total(Shape& s) { return s.area(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "total"),
        ["s.area() → one of: src/a.cpp:2, src/a.cpp:5"]
    );
}

#[test]
fn a_subclass_naming_its_base_through_an_alias_may_override_a_typed_receivers_method() {
    let src = "struct Lamp {\n    virtual int glow() { return 0; }\n};\ntypedef Lamp LampBase;\ntypedef LampBase AnyLamp;\nstruct Torch : AnyLamp {\n    int glow() override { return 2; }\n};\nstruct Flare : Torch {\n    int glow() override { return 3; }\n};\nstruct Dial {\n    virtual int turn() { return 0; }\n};\nusing DialBase = Dial;\nstruct Knob : DialBase {\n    int turn() override { return 1; }\n};\nstruct Bell {\n    virtual int ring() { return 0; }\n};\nint use(Lamp* l, Dial& d, Bell* b) { return l->glow() + d.turn() + b->ring(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "use"),
        [
            "l->glow() → one of: src/a.cpp:2, src/a.cpp:7, src/a.cpp:10, or outside",
            "d.turn() → one of: src/a.cpp:13, src/a.cpp:17, or outside",
            "b->ring() → src/a.cpp:20",
        ]
    );
}

#[test]
fn a_pure_virtual_method_called_on_a_typed_receiver_is_its_override() {
    let src = "struct Shape {\n    virtual int area() = 0;\n};\nstruct Square : Shape {\n    int area() override { return 1; }\n};\nint total(Shape* s) { return s->area(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "total"),
        ["s->area() → src/a.cpp:5"]
    );
}

#[test]
fn a_typed_receiver_lacking_the_method_is_outside_under_an_outside_base() {
    let src = "struct Shape : gfx::Node {};\nstruct Ink {\n    void paint() {}\n};\nvoid run(Shape& s) { s.paint(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["s.paint() → outside"]
    );
}

#[test]
fn a_typed_receiver_lacking_the_method_under_project_bases_is_any_project_method() {
    let src = "struct Shape {};\nstruct Ink {\n    void paint() {}\n};\nvoid run(Shape& s) { s.paint(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["s.paint() → one of: src/a.cpp:3, or outside"]
    );
}

#[test]
fn a_std_typed_receiver_is_outside_beside_a_project_method_of_that_name() {
    let src = "struct Stack {\n    void push_back(int v) {}\n};\nvoid run(std::vector<int>& v) { v.push_back(1); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["v.push_back(1) → outside"]
    );
}

#[test]
fn an_outside_typed_receiver_may_be_a_project_class_deriving_from_it() {
    let src = "struct Fault : std::exception {\n    const char* what() const noexcept override { return \"f\"; }\n};\nvoid run() {\n    try {\n    } catch (const std::exception& e) {\n        e.what();\n    }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["e.what() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_receiver_with_no_type_is_any_project_method_of_that_name() {
    let src = "struct Ink {\n    void paint() {}\n    void show() { auto x = make(); x.paint(); (*this).paint(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "show"),
        [
            "make() → outside",
            "x.paint() → one of: src/a.cpp:2, or outside",
            "(*this).paint() → one of: src/a.cpp:2, or outside"
        ]
    );
}

#[test]
fn a_nested_class_reaches_a_member_of_its_project_base() {
    let h = "struct Ctx {\n    int make() const;\n};\nstruct Model {\n    struct Graph : public Ctx {\n        Graph(int p);\n    };\n};\n";
    let src = "#include \"model.h\"\nint Ctx::make() const { return 0; }\nModel::Graph::Graph(int p) { make(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/model.h", h), ("src/model.cpp", src)],
            "src/model.cpp",
            "Graph"
        ),
        ["make() → src/model.cpp:2"]
    );
}

#[test]
fn a_method_of_a_class_whose_head_no_file_shows_may_call_an_inherited_name() {
    let src = "int flush(int n) { return n; }\nvoid Gen::run() { flush(1); }\n";
    assert_eq!(
        cpp_targets(&[("src/gen.cpp", src)], "src/gen.cpp", "run"),
        ["flush(1) → one of: src/gen.cpp:1, or outside"]
    );
}

#[test]
fn a_receiver_typed_by_a_project_alias_is_any_project_method_of_that_name() {
    let src = "struct Book {\n    void apply() {}\n};\nusing BookPtr = std::unique_ptr<Book>;\nvoid run(BookPtr p) { p->apply(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["p->apply() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_class_is_told_from_a_namesake_in_another_class_or_namespace() {
    let src = "struct Base {\n    void fold() {}\n};\nstruct Left {\n    struct Node {\n        void fold() {}\n    };\n};\nstruct Right {\n    struct Node : Base {\n        void run() { fold(); }\n    };\n};\nnamespace x {\nstruct Cfg {\n    void fold() {}\n};\n}\nnamespace y {\nstruct Cfg : Base {\n    void load() { fold(); }\n};\n}\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["fold() → src/a.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "load"),
        ["fold() → src/a.cpp:2"]
    );
}

#[test]
fn a_call_qualified_by_the_global_scope_is_no_member_call() {
    let src = "int flush(int n) { return n; }\nstruct Sink : lib::Stream {\n    void run() { ::flush(1); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["::flush(1) → src/a.cpp:1"]
    );
}

#[test]
fn a_call_qualified_by_a_class_name_two_classes_share_is_the_one_in_view() {
    let src = "namespace x {\nstruct Cfg {\n    static int load() { return 1; }\n};\nint run() { return Cfg::load(); }\n}\nnamespace y {\nstruct Cfg {\n    static int load() { return 2; }\n};\n}\nint other() { return Cfg::load(); }\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["Cfg::load() → src/a.cpp:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "other"),
        ["Cfg::load() → one of: src/a.cpp:3, src/a.cpp:9, or outside"]
    );
}

#[test]
fn a_call_qualified_by_a_shared_class_name_only_one_class_answers_is_open() {
    let src = "namespace x {\nstruct Cfg {\n    static int load() { return 1; }\n};\n}\nnamespace y {\nstruct Cfg {};\n}\nint other() { return Cfg::load(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "other"),
        ["Cfg::load() → one of: src/a.cpp:3, or outside"]
    );
}

#[test]
fn a_call_qualified_by_an_owner_no_head_shows_is_its_member_or_outside() {
    let src = "int Gen::make() { return 1; }\nint run() { return Gen::make(); }\n";
    assert_eq!(
        cpp_targets(&[("src/gen.cpp", src)], "src/gen.cpp", "run"),
        ["Gen::make() → one of: src/gen.cpp:1, or outside"]
    );
}

#[test]
fn a_call_qualified_by_a_namespace_a_class_elsewhere_is_named_like_reaches_its_function() {
    let src = "namespace ui {\nnamespace dial {\nvoid start() {}\n}\n}\nstruct dial {};\nvoid run() { ui::dial::start(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["ui::dial::start() → src/a.cpp:3"]
    );
}

#[test]
fn a_call_qualified_by_a_project_alias_is_any_project_method_of_that_name() {
    let src = "struct Doc {\n    static int parse() { return 0; }\n};\nusing Text = Doc;\nint run() { return Text::parse(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["Text::parse() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn classes_in_two_files_anonymous_namespaces_are_two_classes() {
    let a =
        "namespace {\nstruct Printer {\n    void line() {}\n    void run() { line(); }\n};\n}\n";
    let b = "namespace {\nstruct Printer : public lib::Out {\n    void go() { line(); }\n};\n}\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["line() → src/a.cpp:3"]
    );
    assert_eq!(cpp_targets(&files, "src/b.cpp", "go"), ["line() → outside"]);
}

#[test]
fn a_class_name_two_programs_define_is_the_one_the_callers_file_sees() {
    let h = "struct Config {\n    void load();\n    void run();\n};\n";
    let src = "#include \"config.h\"\nvoid Config::load() {}\nvoid Config::run() { load(); }\n";
    let tool = "struct Config : lib::Out {\n    void go() { load(); }\n};\n";
    let files = [
        ("app/config.h", h),
        ("app/config.cpp", src),
        ("tool/tool.cpp", tool),
    ];
    assert_eq!(
        cpp_targets(&files, "app/config.cpp", "run"),
        ["load() → app/config.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "tool/tool.cpp", "go"),
        ["load() → outside"]
    );
}

#[test]
fn an_out_of_line_method_written_short_after_a_using_is_its_namespaces_class() {
    let a = "namespace a {\nstruct Node {\n    void tick() {}\n    void step();\n};\n}\n";
    let b = "namespace b {\nstruct Node {\n    void tick();\n};\n}\n";
    let a_src = "#include \"a.h\"\nusing namespace a;\nvoid Node::step() { tick(); }\n";
    let b_src = "#include \"b.h\"\nusing namespace b;\nvoid Node::tick() {}\n";
    let files = [
        ("src/a.h", a),
        ("src/b.h", b),
        ("src/a.cpp", a_src),
        ("src/b.cpp", b_src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → src/a.h:3"]
    );
}

#[test]
fn a_type_name_in_a_class_is_first_a_class_nested_in_its_bases() {
    let src = "struct Cfg {\n    static int load() { return 1; }\n};\nnamespace n {\nstruct Base {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\nstruct App : Base {\n    int run() { return Cfg::load(); }\n    struct Part {\n        int read() { return Cfg::load(); }\n    };\n};\n}\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["Cfg::load() → src/a.cpp:7"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "read"),
        ["Cfg::load() → src/a.cpp:7"]
    );
}

#[test]
fn a_type_name_in_a_class_template_is_not_looked_up_in_a_dependent_base() {
    let src = "struct Slot {\n    void go() {}\n};\ntemplate <class T> struct Rack {\n    struct Slot {\n        void go() {}\n    };\n};\nstruct Shelf {\n    struct Slot {\n        void go() {}\n    };\n};\ntemplate <class T> struct Deep : Rack<T> {\n    void deep() { Slot s; s.go(); }\n};\nstruct Flat : Shelf {\n    void flat() { Slot s; s.go(); }\n};\ntemplate <class U> struct Wrap : Rack<U> {};\ntemplate <class T> struct Mid : Wrap<int> {\n    void mid() { Slot s; s.go(); }\n};\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "deep"),
        ["s.go() → src/a.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "flat"),
        ["s.go() → src/a.cpp:11"]
    );
    // `Wrap<int>` is no dependent base: what it derives from is searched.
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "mid"),
        ["s.go() → src/a.cpp:6"]
    );
}

#[test]
fn a_typed_receiver_in_a_class_with_an_outside_base_is_its_project_class() {
    let src = "struct Cfg {\n    int load() { return 1; }\n};\nstruct App : lib::Base {\n    int run(Cfg& c) { return c.load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.load() → src/a.cpp:2"]
    );
}

#[test]
fn an_anonymous_namespace_class_is_not_a_global_namesake_its_file_sees() {
    let b = "struct Helper : lib::Out {};\n";
    let a = "#include \"b.h\"\nnamespace {\nstruct Helper {\n    void go() {}\n    void run() { go(); }\n};\n}\n";
    assert_eq!(
        cpp_targets(&[("src/b.h", b), ("src/a.h", a)], "src/a.h", "run"),
        ["go() → src/a.h:4"]
    );
}

#[test]
fn a_bare_call_reaching_only_a_dependent_base_may_be_the_free_function() {
    let src = "void flush() {}\ntemplate <class T>\nstruct Base {\n    void flush() {}\n};\ntemplate <class T>\nstruct Sink : Base<T> {\n    void run() { flush(); }\n};\nstruct Pipe : Base<int> {\n    void pump() { flush(); }\n};\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["flush() → one of: src/a.cpp:1, src/a.cpp:4"]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "pump"),
        ["flush() → src/a.cpp:4"]
    );
}

#[test]
fn a_base_is_named_from_outside_its_class() {
    let src = "struct Outer {\n    struct Part {\n        void fit() {}\n    };\n    struct Whole : Part {\n        struct Part {};\n        void make() { fit(); }\n    };\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "make"),
        ["fit() → src/a.cpp:3"]
    );
}

#[test]
fn an_out_of_line_method_of_a_class_no_head_settles_has_no_certain_member() {
    let a = "namespace a {\nstruct Node {\n    void tick() {}\n    void step();\n};\n}\n";
    let b = "namespace b {\nstruct Node {\n    void tick();\n};\n}\n";
    let a_src = "#include \"a.h\"\n#include \"b.h\"\nvoid Node::step() { tick(); }\n";
    let b_src = "#include \"b.h\"\nvoid Node::tick() {}\n";
    let files = [
        ("src/a.h", a),
        ("src/b.h", b),
        ("src/a.cpp", a_src),
        ("src/b.cpp", b_src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → one of: src/a.h:3, src/b.cpp:2, or outside"]
    );
}

#[test]
fn a_class_two_programs_split_into_header_and_source_is_the_callers_programs() {
    let h = "struct Config {\n    void load();\n    void run();\n};\n";
    let src = "#include \"config.h\"\nvoid Config::load() {}\nvoid Config::run() { load(); }\n";
    let other = "#include \"config.h\"\nvoid Config::load() {}\n";
    let files = [
        ("app/config.h", h),
        ("app/config.cpp", src),
        ("tool/config.h", "struct Config {\n    void load();\n};\n"),
        ("tool/config.cpp", other),
    ];
    assert_eq!(
        cpp_targets(&files, "app/config.cpp", "run"),
        ["load() → app/config.cpp:2"]
    );
}

#[test]
fn a_class_names_itself_inside_its_body_under_an_outside_base() {
    let src = "struct Reader : lib::Base {\n    static int peek() { return 0; }\n    int next() { return Reader::peek(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "next"),
        ["Reader::peek() → src/a.cpp:2"]
    );
}

#[test]
fn a_type_name_in_a_class_whose_base_no_head_places_is_not_placed_certainly() {
    let src = "struct Cfg {\n    static int load() { return 1; }\n};\nusing Mixin = lib::Mixin;\nstruct App : Mixin {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["Cfg::load() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_base_named_like_the_class_but_qualified_by_an_outer_namespace_is_outside() {
    let src = "struct Cfg {\n    int load() { return 1; }\n};\nnamespace lib {\nnamespace front {\nnamespace mine {\nstruct Ctx : front::Ctx {\n    int run(Cfg& c) { return c.load(); }\n};\n}\n}\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.load() → src/a.cpp:2"]
    );
}

#[test]
fn a_base_several_project_classes_may_be_hides_type_names_from_certainty() {
    let src = "struct Cfg {\n    static int load() { return 1; }\n};\nnamespace a {\nstruct Node {};\n}\nnamespace b {\nstruct Node {};\n}\nstruct App : Node {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["Cfg::load() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn an_out_of_line_method_of_an_anonymous_class_is_not_a_global_namesakes() {
    let a = "namespace {\nstruct Unit {\n    void step();\n    void tick() {}\n};\n}\nvoid Unit::step() { tick(); }\n";
    let b = "struct Unit {\n    void tick() {}\n};\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → src/a.cpp:4"]
    );
}

#[test]
fn a_type_name_in_a_files_code_is_its_own_anonymous_class_not_an_unseen_global() {
    let a = "struct Pen {\n    void draw() {}\n};\n";
    let b = "namespace {\nstruct Pen {\n    void draw() {}\n};\nvoid first() { Pen p; p.draw(); }\n}\nvoid second() { Pen q; q.draw(); }\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "first"),
        ["p.draw() → src/b.cpp:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "second"),
        ["q.draw() → src/b.cpp:3"]
    );
}

#[test]
fn a_type_name_no_file_in_view_defines_is_outside() {
    let b = "namespace {\nstruct Pen {\n    void draw() {}\n};\n}\n";
    let c = "#include <lib/pen.h>\nvoid run(Pen& p) { p.draw(); }\n";
    assert_eq!(
        cpp_targets(&[("src/b.cpp", b), ("src/c.cpp", c)], "src/c.cpp", "run"),
        ["p.draw() → outside"]
    );
}

#[test]
fn a_qualifier_is_placed_through_the_bases_the_callers_file_sees() {
    let common =
        "struct Base {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n";
    let app = "#include \"common.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App {\n    int run() { return Cfg::load(); }\n};\n";
    let tool = "#include \"common.h\"\nstruct App : Base {\n    int go() { return 0; }\n};\n";
    let files = [
        ("src/common.h", common),
        ("src/app.cpp", app),
        ("src/tool.cpp", tool),
    ];
    assert_eq!(
        cpp_targets(&files, "src/app.cpp", "run"),
        ["Cfg::load() → src/app.cpp:3"]
    );
}

#[test]
fn a_files_anonymous_class_and_a_global_namesake_it_sees_are_not_told_apart() {
    let g = "struct Pen {\n    void draw() {}\n};\n";
    let b = "#include \"pen.h\"\nnamespace {\nstruct Pen {\n    void draw() {}\n};\nvoid first() { Pen p; p.draw(); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/pen.h", g), ("src/b.cpp", b)], "src/b.cpp", "first"),
        ["p.draw() → one of: src/b.cpp:4, src/pen.h:2, or outside"]
    );
}

#[test]
fn a_base_the_project_declares_as_an_alias_is_not_read_as_outside() {
    let h = "namespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\nusing Base = detail::Impl;\n}\n";
    let src = "#include \"a.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nnamespace other {\nstruct Base {};\n}\nstruct App : lib::Base {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:4, src/b.cpp:3, or outside"]
    );
}

#[test]
fn anonymous_classes_of_two_files_with_out_of_line_methods_are_each_their_own() {
    let a = "namespace {\nstruct Impl {\n    void tick();\n    void step();\n};\nvoid Impl::tick() {}\nvoid Impl::step() { tick(); }\n}\n";
    let b = "namespace {\nstruct Impl {\n    void tick();\n    void step();\n};\nvoid Impl::tick() {}\nvoid Impl::step() { tick(); }\n}\n";
    let files = [("src/a.cpp", a), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "step"),
        ["tick() → src/a.cpp:6"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "step"),
        ["tick() → src/b.cpp:6"]
    );
}

#[test]
fn a_class_an_includer_provides_is_not_hidden_by_a_namesake_the_header_sees() {
    let n = "namespace n {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let usage = "#include \"g.h\"\nnamespace n {\ninline void f() { Cfg c; c.load(); }\n}\n";
    let main = "#include \"n.h\"\n#include \"use.h\"\nint main() { return 0; }\n";
    let files = [
        ("src/n.h", n),
        ("src/g.h", g),
        ("src/use.h", usage),
        ("src/main.cpp", main),
    ];
    assert_eq!(
        cpp_targets(&files, "src/use.h", "f"),
        ["c.load() → one of: src/g.h:2, src/n.h:3"]
    );
}

#[test]
fn a_class_out_of_view_that_lookup_meets_first_keeps_the_one_in_view_uncertain() {
    let n = "namespace n {\nstruct Cfg {\n    static int load() { return 2; }\n    int size() { return 0; }\n};\n}\n";
    let b = "#include <n.h>\nstruct Cfg {\n    static int load() { return 1; }\n    int size() { return 1; }\n};\nnamespace n {\nint run() { return Cfg::load(); }\nint go() { Cfg c; return c.size(); }\n}\n";
    let files = [("src/n.h", n), ("src/b.cpp", b)];
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/b.cpp:3, src/n.h:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/b.cpp", "go"),
        ["c.size() → one of: src/b.cpp:4, src/n.h:4"]
    );
}

#[test]
fn a_base_through_an_alias_template_leaves_a_type_name_open() {
    let h = "namespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\ntemplate <class T>\nusing Base = detail::Impl;\n}\n";
    let src = "#include \"a.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App : lib::Base<int> {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:4, src/b.cpp:3, or outside"]
    );
}

#[test]
fn a_base_through_an_alias_out_of_view_leaves_a_type_name_open() {
    let h = "namespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\nusing Base = detail::Impl;\n}\n";
    let src = "#include <a.h>\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App : lib::Base {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:4, src/b.cpp:3, or outside"]
    );
}

#[test]
fn a_base_through_an_alias_of_an_outside_class_leaves_a_type_name_open() {
    let h = "#include <ext/thing.h>\nnamespace lib {\nusing Base = ext::Thing;\n}\nnamespace other {\nstruct Cfg {\n    static int load() { return 2; }\n};\n}\n";
    let src = "#include \"a.h\"\nstruct Cfg {\n    static int load() { return 1; }\n};\nstruct App : lib::Base {\n    int run() { return Cfg::load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.h", h), ("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/a.h:7, src/b.cpp:3, or outside"]
    );
}

#[test]
fn an_alias_met_before_a_global_class_of_its_name_leaves_the_base_unplaced() {
    let src = "struct Base {};\nstruct Cfg {\n    static int load() { return 1; }\n};\nnamespace detail {\nstruct Impl {\n    struct Cfg {\n        static int load() { return 2; }\n    };\n};\n}\nnamespace lib {\nusing Base = detail::Impl;\nstruct App : Base {\n    int run() { return Cfg::load(); }\n};\n}\n";
    assert_eq!(
        cpp_targets(&[("src/b.cpp", src)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/b.cpp:3, src/b.cpp:8, or outside"]
    );
}

#[test]
fn a_qualifier_either_class_may_be_is_never_certain() {
    let n = "#include <ext/base.h>\nnamespace n {\nstruct Cfg : ext::Base {};\n}\n";
    let b = "#include <n.h>\nstruct Cfg {\n    static int load() { return 1; }\n};\nnamespace n {\nint run() { return Cfg::load(); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/n.h", n), ("src/b.cpp", b)], "src/b.cpp", "run"),
        ["Cfg::load() → one of: src/b.cpp:3, or outside"]
    );
}

#[test]
fn a_qualified_call_of_a_member_an_outside_base_gives_is_outside() {
    let n = "#include <ext/base.h>\nnamespace n {\nstruct Cfg : ext::Base {};\n}\n";
    let b = "#include \"n.h\"\nnamespace n {\nint run() { return Cfg::load(); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/n.h", n), ("src/b.cpp", b)], "src/b.cpp", "run"),
        ["Cfg::load() → outside"]
    );
}

#[test]
fn a_typedef_of_a_qualified_name_is_an_alias() {
    let src = "struct Cfg {\n    void load() {}\n};\nstruct App {\n    typedef ext::Cfg Cfg;\n    void run() { Cfg c; c.load(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.load() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn an_alias_whose_path_ends_in_the_name_leaves_the_last_step_open() {
    let u = "namespace lib {\nusing Cfg = ext::Thing;\n}\nusing namespace lib;\n";
    let o = "namespace other {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let main = "#include \"u.h\"\n#include \"o.h\"\nvoid run() { Cfg c; c.load(); }\n";
    let files = [("src/u.h", u), ("src/o.h", o), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        ["c.load() → one of: src/o.h:3, or outside"]
    );
}

#[test]
fn an_alias_in_another_source_file_does_not_reach_the_caller() {
    let h = "struct Buffer {\n    void clear() {}\n};\n";
    let a = "#include \"buf.h\"\nvoid run(Buffer& b) { b.clear(); }\n";
    let other = "using Buffer = std::vector<char>;\n";
    let files = [("src/buf.h", h), ("src/a.cpp", a), ("src/other.cpp", other)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        ["b.clear() → src/buf.h:2"]
    );
}

#[test]
fn a_using_declaration_of_a_class_places_its_name() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"a.h\"\nusing a::Cfg;\nvoid run() { Cfg c; c.load(); }\n";
    let files = [("src/a.h", a), ("src/g.h", g), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "run"),
        ["c.load() → src/a.h:3"]
    );
}

#[test]
fn a_qualifier_either_class_may_be_stays_open_when_one_class_has_none_of_its_own() {
    let n = "#include <ext/base.h>\nnamespace n {\nstruct Cfg : ext::Base {};\n}\n";
    let b = "#include <n.h>\nstruct Cfg {\n    static int load(int a) { return a; }\n    static int load(double a) { return 0; }\n};\nnamespace n {\nint run() { return Cfg::load(1); }\n}\n";
    assert_eq!(
        cpp_targets(&[("src/n.h", n), ("src/b.cpp", b)], "src/b.cpp", "run"),
        ["Cfg::load(1) → one of: src/b.cpp:3, src/b.cpp:4, or outside"]
    );
}

#[test]
fn a_using_declaration_inside_a_function_does_not_place_the_name_for_the_file() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"a.h\"\n#include \"g.h\"\nvoid f() {\n    using a::Cfg;\n    Cfg c;\n    c.load();\n}\nvoid g() {\n    Cfg c;\n    c.load();\n}\n";
    let files = [("src/a.h", a), ("src/g.h", g), ("src/main.cpp", main)];
    let either = ["c.load() → one of: src/a.h:3, src/g.h:2"];
    assert_eq!(cpp_targets(&files, "src/main.cpp", "f"), either);
    assert_eq!(cpp_targets(&files, "src/main.cpp", "g"), either);
}

#[test]
fn a_using_declaration_inside_a_namespace_does_not_place_the_name_for_the_file() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"a.h\"\n#include \"g.h\"\nnamespace x {\nusing a::Cfg;\n}\nvoid g() {\n    Cfg c;\n    c.load();\n}\n";
    let files = [("src/a.h", a), ("src/g.h", g), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "g"),
        ["c.load() → one of: src/a.h:3, src/g.h:2"]
    );
}

#[test]
fn a_file_scope_using_declaration_of_an_outside_class_makes_the_name_outside() {
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include <ext/cfg.h>\nusing ext::Cfg;\nvoid run() { Cfg c; c.load(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/g.h", g), ("src/main.cpp", main)],
            "src/main.cpp",
            "run"
        ),
        ["c.load() → outside"]
    );
}

#[test]
fn a_using_directive_inside_a_function_does_not_place_the_name_for_the_file() {
    let a = "namespace a {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let b = "namespace b {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let main = "#include \"a.h\"\n#include \"b.h\"\nvoid f() {\n    using namespace a;\n    Cfg c;\n    c.load();\n}\nvoid g() {\n    Cfg c;\n    c.load();\n}\n";
    let files = [("src/a.h", a), ("src/b.h", b), ("src/main.cpp", main)];
    assert_eq!(
        cpp_targets(&files, "src/main.cpp", "g"),
        ["c.load() → one of: src/a.h:3, src/b.h:3, or outside"]
    );
}

#[test]
fn an_inherited_constructor_using_leaves_the_base_placed() {
    let src = "struct Base {\n    void go() {}\n};\nstruct Kid : Base {\n    using Base::Base;\n    void run() { go(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["go() → src/a.cpp:2"]
    );
}

#[test]
fn a_using_declaration_of_a_global_class_inside_a_namespace_places_it() {
    let g = "struct Cfg {\n    void load() {}\n};\n";
    let main = "#include \"g.h\"\nnamespace n {\nusing ::Cfg;\nvoid f() { Cfg c; c.load(); }\n}\n";
    assert_eq!(
        cpp_targets(
            &[("src/g.h", g), ("src/main.cpp", main)],
            "src/main.cpp",
            "f"
        ),
        ["c.load() → src/g.h:2"]
    );
}

#[test]
fn a_using_declaration_through_a_project_namespace_alias_is_not_outside() {
    let d = "namespace proj { namespace detail {\nstruct Cfg {\n    void load() {}\n};\n} }\n";
    let main = "#include \"d.h\"\nnamespace p = proj::detail;\nusing p::Cfg;\nvoid f() { Cfg c; c.load(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/d.h", d), ("src/main.cpp", main)],
            "src/main.cpp",
            "f"
        ),
        ["c.load() → one of: src/d.h:3, or outside"]
    );
}

#[test]
fn a_field_of_a_project_type_is_certain_with_or_without_this() {
    let h =
        "#include \"book.h\"\nstruct Shelf {\n    void run();\n    const Book* top_, *low_;\n};\n";
    let book = "struct Book {\n    void apply() {}\n};\n";
    let src = "#include \"shelf.h\"\nvoid Shelf::run() {\n    top_->apply();\n    this->low_->apply();\n    auto go = [this] { top_->apply(); };\n}\n";
    let files = [
        ("src/book.h", book),
        ("src/shelf.h", h),
        ("src/shelf.cpp", src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/shelf.cpp", "run"),
        [
            "top_->apply() → src/book.h:2",
            "this->low_->apply() → src/book.h:2",
            "top_->apply() → src/book.h:2"
        ]
    );
}

#[test]
fn a_field_of_a_type_a_subclass_overrides_is_one_of_both() {
    let src = "struct Shape {\n    virtual int area() { return 0; }\n};\nstruct Square : Shape {\n    int area() override { return 1; }\n};\nstruct Canvas {\n    Shape* shape_;\n    int run() { return shape_->area(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["shape_->area() → one of: src/a.cpp:2, src/a.cpp:5"]
    );
}

#[test]
fn a_data_member_called_through_a_typed_receiver_is_not_visible() {
    let c = "struct port {\n    int (*send)(int);\n};\nint flush(struct port *p) { return p->send(3); }\n";
    let cpp = "#include <functional>\n#include <relay.h>\nstruct Valve : Relay {\n    void (*shut)(int);\n    std::function<void(int)> on_drip;\n    int open() { return 1; }\n};\nint drain(Valve& v) {\n    v.shut(2);\n    v.on_drip(1);\n    return v.open();\n}\n";
    assert_eq!(
        cpp_targets(&[("src/port.c", c)], "src/port.c", "flush"),
        ["p->send(3) → hidden"]
    );
    assert_eq!(
        cpp_targets(&[("src/valve.cpp", cpp)], "src/valve.cpp", "drain"),
        [
            "v.shut(2) → hidden",
            "v.on_drip(1) → hidden",
            "v.open() → src/valve.cpp:6"
        ]
    );
}

#[test]
fn a_static_data_member_called_through_its_class_is_not_visible() {
    let src = "#include <relay.h>\nstruct Log : Relay {\n    static void (*sink)(const char*);\n};\nvoid say() { Log::sink(\"x\"); }\n";
    assert_eq!(
        cpp_targets(&[("src/log.cpp", src)], "src/log.cpp", "say"),
        ["Log::sink(\"x\") → hidden"]
    );
}

#[test]
fn a_call_on_a_calls_result_is_not_a_method_of_the_receivers_class() {
    let src = "struct Leaf {\n    int next() { return 0; }\n};\nstruct Node {\n    Leaf* next() { return nullptr; }\n};\nint walk(Node* n) { return n->next()->next(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "walk"),
        [
            "n->next() → src/a.cpp:5",
            "n->next()->next() → one of: src/a.cpp:2, src/a.cpp:5, or outside"
        ]
    );
}

#[test]
fn a_call_through_a_path_of_data_members_reads_the_last_member() {
    let c = "struct ops {\n    int (*shut)(int);\n};\nstruct dev {\n    struct ops table;\n};\nint stop(struct dev *d) { return d->table.shut(4); }\n";
    let cpp = "#include <utility>\nstruct Pump {\n    int (*prime)(int);\n    int start() { return 0; }\n};\nstruct Rig {\n    Pump pump;\n    int boot() { return this->pump.prime(1); }\n};\nint run(Rig* r) {\n    r->pump.prime(2);\n    return r->pump.start();\n}\nint spare(std::pair<Pump, int>& q) { return q.first.start(); }\n";
    assert_eq!(
        cpp_targets(&[("src/dev.c", c)], "src/dev.c", "stop"),
        ["d->table.shut(4) → hidden"]
    );
    assert_eq!(
        cpp_targets(&[("src/rig.cpp", cpp)], "src/rig.cpp", "run"),
        [
            "r->pump.prime(2) → hidden",
            "r->pump.start() → src/rig.cpp:4"
        ]
    );
    assert_eq!(
        cpp_targets(&[("src/rig.cpp", cpp)], "src/rig.cpp", "boot"),
        ["this->pump.prime(1) → hidden"]
    );
    // A member of a type from outside the project may be a project class.
    assert_eq!(
        cpp_targets(&[("src/rig.cpp", cpp)], "src/rig.cpp", "spare"),
        ["q.first.start() → one of: src/rig.cpp:4, or outside"]
    );
}

#[test]
fn a_smart_pointer_field_is_outside_with_a_dot_and_its_pointee_with_an_arrow() {
    let src = "struct Book {\n    void apply() {}\n    void reset() {}\n};\nstruct Shelf {\n    std::unique_ptr<Book> book_ = nullptr;\n    void run() {\n        book_->apply();\n        this->book_->apply();\n        book_.reset();\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "book_->apply() → src/a.cpp:2",
            "this->book_->apply() → src/a.cpp:2",
            "book_.reset() → outside"
        ]
    );
}

#[test]
fn a_field_a_project_base_declares_is_typed_by_it() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Base {\n    Book book_;\n};\nstruct Mid : Base {};\nstruct Shelf : public Mid {\n    void run() { book_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → src/a.cpp:2"]
    );
}

#[test]
fn a_field_behind_a_nearer_base_no_head_places_is_not_certain() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Top {\n    Book book_;\n};\nstruct Mid : Top {};\nusing Other = Ink;\nstruct Shelf : Other, Mid {\n    void run() { book_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → one of: src/a.cpp:2, src/a.cpp:5, or outside"]
    );
}

#[test]
fn a_function_field_called_bare_is_hidden_beside_a_free_function_of_its_name() {
    let src = "void notify(int v) {}\nstruct Shelf {\n    std::function<void(int)> notify;\n    void run() { notify(1); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["notify(1) → hidden"]
    );
}

#[test]
fn a_local_named_like_a_field_is_the_local() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Shelf {\n    Book book_;\n    void run(Ink book_) { book_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → src/a.cpp:5"]
    );
}

#[test]
fn a_field_type_is_placed_from_the_class_not_from_the_callers_file() {
    let h = "struct Cfg {\n    void load() {}\n};\nstruct Shelf {\n    Cfg cfg_;\n    void run();\n};\n";
    let b = "namespace b {\nstruct Cfg {\n    void load() {}\n};\n}\n";
    let src = "#include \"shelf.h\"\n#include \"b.h\"\nusing namespace b;\nvoid Shelf::run() { cfg_.load(); }\n";
    let files = [("src/shelf.h", h), ("src/b.h", b), ("src/shelf.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/shelf.cpp", "run"),
        ["cfg_.load() → src/shelf.h:2"]
    );
}

#[test]
fn a_field_typed_by_a_template_parameter_is_no_project_class() {
    let src = "struct Book {\n    void apply() {}\n};\ntemplate <class Book>\nstruct Box {\n    Book item_;\n    void run() { item_.apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["item_.apply() → one of: src/a.cpp:2, or outside"]
    );
}

#[test]
fn a_method_returning_a_pointer_or_reference_is_no_data_member() {
    let src = "struct Shelf {\n    int* top();\n    const int& low() const;\n    int (*pick_)(int);\n    void run() { top(); low(); pick_(1); }\n};\nint* Shelf::top() { return nullptr; }\nconst int& Shelf::low() const { return *top(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "top() → src/a.cpp:7",
            "low() → src/a.cpp:8",
            "pick_(1) → hidden"
        ]
    );
}

#[test]
fn a_smart_pointer_of_a_template_parameter_is_no_project_class() {
    let src = "struct Engine {\n    void start() {}\n};\ntemplate <class Engine>\nvoid boot(std::unique_ptr<Engine> e) { e->start(); }\ntemplate <class Engine>\nstruct Pad {\n    void run(std::shared_ptr<Engine> e) { e->start(); }\n};\n";
    let open = ["e->start() → one of: src/a.cpp:2, or outside"];
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "boot"),
        open
    );
    assert_eq!(cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"), open);
    let late = "struct Engine {\n    void start() {}\n};\ntemplate <class Engine>\nvoid boot(std::unique_ptr<Engine const> e) { e->start(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", late)], "src/a.cpp", "boot"),
        open
    );
}

#[test]
fn a_project_class_with_a_template_parameter_argument_keeps_its_type() {
    let src = "template <class T>\nstruct Box {\n    int size() { return 0; }\n};\nstruct Tray {\n    int size() { return 1; }\n};\ntemplate <class T>\nint count(Box<T>& b) { return b.size(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "count"),
        ["b.size() → src/a.cpp:3"]
    );
}

#[test]
fn a_member_in_a_class_local_to_a_method_is_not_the_methods_class_field() {
    let src = "struct Lock {\n    void unlock() {}\n};\nstruct Spin {\n    void unlock() {}\n};\nstruct Shelf {\n    Lock mutex_;\n    std::function<void()> notify;\n    void run() {\n        struct Guard {\n            Spin& mutex_;\n            void notify() {}\n            ~Guard() { mutex_.unlock(); notify(); }\n        };\n        mutex_.unlock();\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "mutex_.unlock() → one of: src/a.cpp:2, src/a.cpp:5, or outside",
            "notify() → outside",
            "mutex_.unlock() → src/a.cpp:2"
        ]
    );
}

#[test]
fn an_arrow_on_a_class_with_an_arrow_operator_is_not_its_own_method() {
    let src = "struct Book {\n    void reset() {}\n};\nstruct Handle {\n    Book* operator->();\n    void reset() {}\n};\nstruct Base {\n    Book* operator->();\n};\nstruct Ref : Base {\n    void reset() {}\n};\nstruct Shelf {\n    Handle h_;\n    void run(Ref r) { h_->reset(); h_.reset(); r->reset(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "h_->reset() → one of: src/a.cpp:2, src/a.cpp:6, src/a.cpp:12, or outside",
            "h_.reset() → src/a.cpp:6",
            "r->reset() → one of: src/a.cpp:2, src/a.cpp:6, src/a.cpp:12, or outside"
        ]
    );
}

#[test]
fn a_smart_pointer_to_a_const_project_class_is_its_pointee() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Shelf {\n    std::shared_ptr<const Book> w_;\n    std::unique_ptr<Book const> u_;\n    void run(std::unique_ptr<const Book> p) { w_->apply(); u_->apply(); p->apply(); }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "w_->apply() → src/a.cpp:2",
            "u_->apply() → src/a.cpp:2",
            "p->apply() → src/a.cpp:2"
        ]
    );
}

#[test]
fn an_optional_of_a_project_class_is_its_value_through_the_arrow() {
    let src = "struct Lamp {\n    void glow(int n) {}\n    void reset() {}\n};\nstruct Holder {\n    std::optional<Lamp> lamp;\n    void run() {\n        lamp->glow(7);\n        lamp.reset();\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["lamp->glow(7) → src/a.cpp:2", "lamp.reset() → outside"]
    );
}

#[test]
fn a_data_member_named_to_a_project_macro_may_be_a_local_the_macro_declares() {
    // `DECLARE_BOX(b)` declares a local `b` of another type than the member.
    let h = "struct Red { void tick(int v) {} };\nstruct Blue { void tick(int v) {} };\nstruct RedBox { Red g; };\nstruct BlueBox { Blue g; };\n#define DECLARE_BOX(n) BlueBox n\n#define DECLARE_AS(t, a, n) a t n\nvoid note(RedBox& r);\nstruct Host {\n    RedBox b;\n    RedBox c;\n    void shadowed() { DECLARE_BOX(b); b.g.tick(1); c.g.tick(2); }\n    void listed() { DECLARE_AS(BlueBox, alignas(16) /* kept apart from the red ones, which stay members */, *c); c->g.tick(4); }\n    void passed() { note(c); c.g.tick(3); }\n};\n";
    let files = [("src/box.h", h)];
    // The name is read past the 80 characters a call's text shows.
    assert_eq!(
        cpp_targets(&files, "src/box.h", "listed").last().unwrap(),
        "c->g.tick(4) → one of: src/box.h:1, src/box.h:2, or outside"
    );
    assert_eq!(
        cpp_targets(&files, "src/box.h", "shadowed"),
        [
            "DECLARE_BOX(b) → hidden",
            "b.g.tick(1) → one of: src/box.h:1, src/box.h:2, or outside",
            "c.g.tick(2) → src/box.h:1"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/box.h", "passed"),
        ["note(c) → src/box.h:7", "c.g.tick(3) → src/box.h:1"]
    );
}

#[test]
fn a_function_like_macro_block_in_a_method_keeps_the_methods_fields() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Shelf {\n    Book book_;\n    std::vector<int> items_;\n    void run() {\n        EACH_ITEM(int i, items_) { book_.apply(); }\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["book_.apply() → src/a.cpp:2"]
    );
}

#[test]
fn a_call_in_a_local_class_method_is_never_certainly_the_enclosing_classs_method() {
    let src = "void tidy() {}\nstruct Shelf {\n    void flush() {}\n    void tidy() {}\n    void run() {\n        struct Guard {\n            void flush() {}\n            ~Guard() { flush(); this->flush(); ::tidy(); tidy(); }\n        };\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "flush() → one of: src/a.cpp:3, or outside",
            "this->flush() → one of: src/a.cpp:3, or outside",
            "::tidy() → src/a.cpp:1",
            "tidy() → one of: src/a.cpp:1, src/a.cpp:4, or outside"
        ]
    );
}

#[test]
fn an_arrow_operator_declared_with_spaces_is_found() {
    let src = "struct Book {\n    void reset() {}\n};\nstruct Spaced {\n    Book* operator -> ();\n    void reset() {}\n};\nvoid run(Spaced s) { s->reset(); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["s->reset() → one of: src/a.cpp:2, src/a.cpp:6, or outside"]
    );
}

#[test]
fn an_items_own_flow_marked_as_a_local_class_method_still_reaches_its_class() {
    // Regression pin: in a file whose braces split across `#if`/`#else`, a
    // method charted from the tree that misreads the split sits in a body,
    // so its own flow carries `local_class`; the item is still its class's.
    let src = "struct Turn {\n    void flush() {}\n    void push() { flush(); }\n};\n";
    let mut p = plaz::source::project([("src/a.h".to_string(), src.to_string())]);
    let f = p.files.get_mut("src/a.h").unwrap();
    let at = |name: &str| (f.code.items.iter()).position(|it| it.name == name);
    let (push, flush) = (at("push").unwrap(), at("flush").unwrap());
    f.flows.get_mut(&push).unwrap().local_class = true;
    let resolved = plaz::resolve::resolve(p);
    let mut out = Vec::new();
    resolved.project().files["src/a.h"].flows[&push]
        .each_call(&mut |_, c| out.push(c.target.clone()));
    let flush = plaz::model::FnId {
        path: "src/a.h".to_string(),
        item: flush,
    };
    assert_eq!(out, [plaz::model::Target::Resolved(flush)]);
}

#[test]
fn a_local_class_method_under_if_or_as_a_template_is_still_the_local_classs() {
    let src = "struct Book {\n    void apply() {}\n};\nstruct Ink {\n    void apply() {}\n};\nstruct Shelf {\n    Book book_;\n    void flush() {}\n    void run() {\n        struct Guard {\n            Ink book_;\n#ifdef DEBUG\n            void go() { flush(); book_.apply(); this->flush(); }\n#endif\n            template <class T>\n            void put(T t) { book_.apply(); }\n            void flush() {}\n        };\n    }\n};\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        [
            "flush() → one of: src/a.cpp:9, or outside",
            "book_.apply() → one of: src/a.cpp:2, src/a.cpp:5, or outside",
            "this->flush() → one of: src/a.cpp:9, or outside",
            "book_.apply() → one of: src/a.cpp:2, src/a.cpp:5, or outside"
        ]
    );
}

#[test]
fn a_class_head_with_a_macro_before_its_name_is_a_class() {
    let h = "#define KIT_API\nnamespace kit {\nclass KIT_API Base {\npublic:\n    bool isLit() const;\n};\nclass KIT_API Pane final : public Base {\npublic:\n    class KIT_API Part {\n    public:\n        void tick();\n    };\n    void setQuiet(bool on);\n    void open(int h);\n};\n}\n";
    let src = "#include \"pane.h\"\nnamespace kit {\nbool Base::isLit() const { return true; }\nvoid Pane::setQuiet(bool on) {}\nvoid Pane::open(int h) { setQuiet(h > 0); }\nvoid Pane::Part::tick() {}\n}\nvoid setQuiet(int fd, bool on) {}\nvoid run() {\n    kit::Pane p;\n    p.isLit();\n    kit::Pane::Part q;\n    q.tick();\n}\n";
    let files = [("src/pane.h", h), ("src/pane.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "open"),
        ["setQuiet(h > 0) → src/pane.cpp:4"]
    );
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "run"),
        ["p.isLit() → src/pane.cpp:3", "q.tick() → src/pane.cpp:6"]
    );
}

#[test]
fn a_methods_declaration_stays_its_classs_beside_a_namesake_classs_definition() {
    let h = "struct Shape {\n    int area() const;\n    int twice() const;\n};\n";
    let src = "#include \"shape.h\"\nint Shape::twice() const { return 2 * area(); }\n";
    let other = "namespace {\nstruct Shape {\n    int area() const { return 1; }\n};\n}\n";
    let files = [
        ("src/shape.h", h),
        ("src/shape.cpp", src),
        ("src/other.cpp", other),
    ];
    assert_eq!(
        cpp_targets(&files, "src/shape.cpp", "twice"),
        ["area() → src/shape.h:2"]
    );
}

#[test]
fn a_field_of_a_nested_class_the_head_only_declares_is_placed_where_it_is_defined() {
    let h = "#include <memory>\nclass Pane {\npublic:\n    void show();\nprivate:\n    struct Core;\n    std::unique_ptr<Core> m_core;\n};\n";
    let src = "#include \"pane.h\"\nstruct Pane::Core {\n    void paint() {}\n};\nvoid Pane::show() { m_core->paint(); }\n";
    let files = [("src/pane.h", h), ("src/pane.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "show"),
        ["m_core->paint() → src/pane.cpp:3"]
    );
    // An outside type the caller's file has a namesake of elsewhere is not it.
    let h = "class Pane {\npublic:\n    void show();\nprivate:\n    Gadget* m_gadget;\n};\n";
    let src = "#include \"pane.h\"\nnamespace detail {\nstruct Gadget {\n    void paint() {}\n};\n}\nvoid Pane::show() { m_gadget->paint(); }\n";
    let files = [("src/pane.h", h), ("src/pane.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/pane.cpp", "show"),
        ["m_gadget->paint() → outside"]
    );
}

#[test]
fn an_elaborated_type_with_a_brace_initialiser_is_no_class_head() {
    let src = "struct Clock {\n    void tick();\n};\nvoid Clock::tick() {}\nvoid run() {\n    struct Clock c{};\n    c.tick();\n}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "run"),
        ["c.tick() → src/a.cpp:4"]
    );
}

#[test]
fn a_bare_call_in_a_method_of_a_class_no_head_shows_lists_its_methods_beside_the_free_ones() {
    let src = "#include <kit/pane.h>\nvoid Pane::setQuiet(bool on) {}\nvoid Pane::open(int h) { setQuiet(h > 0); }\nvoid setQuiet(int fd, bool on) {}\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "open"),
        ["setQuiet(h > 0) → one of: src/a.cpp:2, src/a.cpp:4, or outside"]
    );
}

#[test]
fn a_declaration_no_definitions_file_includes_is_any_programs_definition() {
    let entry =
        "extern int main(int argc, char* argv[]);\nint start() { return main(0, nullptr); }\n";
    let one = "int main(int argc, char* argv[]) { return 0; }\n";
    let two = "int main(int argc, char** argv) { return 1; }\n";
    let none = "int main() { return 2; }\n";
    let files = [
        ("lib/entry.cpp", entry),
        ("demo/one.cpp", one),
        ("demo/two.cpp", two),
        ("demo/none.cpp", none),
    ];
    assert_eq!(
        cpp_targets(&files, "lib/entry.cpp", "start"),
        ["main(0, nullptr) → one of: demo/one.cpp:1, demo/two.cpp:1, or outside"]
    );
    // The header and source split: the defining file includes the header.
    let h = "int count(int n);\n";
    let def = "#include \"count.h\"\nint count(int n) { return n; }\n";
    let call = "#include \"count.h\"\nint run() { return count(1); }\n";
    let files = [
        ("src/count.h", h),
        ("src/count.cpp", def),
        ("src/run.cpp", call),
    ];
    assert_eq!(
        cpp_targets(&files, "src/run.cpp", "run"),
        ["count(1) → src/count.cpp:2"]
    );
}

#[test]
fn a_namespace_alias_is_the_namespace_it_names() {
    let h = "namespace kit {\nnamespace ui {\nvoid paint(int n);\n}\n}\n";
    let def = "#include \"ui.h\"\nnamespace kit {\nnamespace ui {\nvoid paint(int n) {}\n}\n}\n";
    let src = "#include \"ui.h\"\nnamespace fs = std::filesystem;\nnamespace kui = ::kit::ui;\nvoid run() {\n    fs::create_directories(\"x\");\n    kui::paint(1);\n}\n";
    let files = [("src/ui.h", h), ("src/ui.cpp", def), ("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        [
            "fs::create_directories(\"x\") → outside",
            "kui::paint(1) → src/ui.cpp:4"
        ]
    );
}

#[test]
fn a_member_or_local_of_a_project_class_called_bare_is_its_call_operator() {
    let src = "#include <functional>\nstruct Less {\n    bool operator()(int a, int b) const { return a < b; }\n};\nstruct Plain {\n    int v;\n};\nstruct Sorter {\n    Less less_;\n    std::function<bool(int, int)> fn_;\n    bool (*ptr_)(int, int);\n    Plain plain_;\n    bool run(int a, int b) { return less_(a, b) && fn_(a, b) && ptr_(a, b) && plain_(a, b); }\n};\nbool pick(int a, int b, Plain p) {\n    Less cmp;\n    return cmp(a, b) && p(a, b);\n}\n";
    let files = [("src/a.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "run"),
        [
            "less_(a, b) → src/a.cpp:3",
            "fn_(a, b) → hidden",
            "ptr_(a, b) → hidden",
            "plain_(a, b) → hidden"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "pick"),
        ["cmp(a, b) → src/a.cpp:3", "p(a, b) → hidden"]
    );
}

#[test]
fn a_class_head_with_a_macro_after_its_name_keeps_its_name() {
    let h = "#define TOY_FINAL final\nstruct Gear {\n    void spin() {}\n};\nclass Crank TOY_FINAL : public Gear {\npublic:\n    void start() { spin(); }\n};\nclass Wheel TOY_FINAL {\npublic:\n    void spin() {}\n};\nvoid roll() {\n    Wheel w;\n    w.spin();\n}\n#define toy_api\nclass toy_api Lever : public Gear {\npublic:\n    void pull() { spin(); }\n};\n";
    let files = [("src/toy.h", h)];
    assert_eq!(
        cpp_targets(&files, "src/toy.h", "start"),
        ["spin() → src/toy.h:3"]
    );
    assert_eq!(
        cpp_targets(&files, "src/toy.h", "roll"),
        ["w.spin() → src/toy.h:11"]
    );
    // A lower-case word a project file defines is the macro too.
    assert_eq!(
        cpp_targets(&files, "src/toy.h", "pull"),
        ["spin() → src/toy.h:3"]
    );
}

#[test]
fn a_class_head_whose_macro_word_cannot_be_told_is_left_unread() {
    // Neither word is a project macro or in capitals: the head stays
    // misread, so its body's method is no item.
    let h = "struct Gear {\n    void spin() {}\n};\nclass Crank quiet : public Gear {\npublic:\n    void start() { spin(); }\n};\nclass Wheel {\npublic:\n    void spin() {}\n};\n";
    let p = plaz::source::project([("src/toy.h".to_string(), h.to_string())]);
    let lines: Vec<usize> = (p.files["src/toy.h"].code.items.iter())
        .filter(|it| it.name == "spin" || it.name == "start")
        .map(|it| it.start_line)
        .collect();
    assert_eq!(lines, [2, 10]);
}

#[test]
fn a_declaration_whose_header_has_an_unread_objective_c_twin_is_open() {
    let h =
        "bool touched(int f);\nstruct Pad {\n    bool held(int f);\n    bool both(int f);\n};\n";
    let def = "#include \"../pad.hpp\"\nbool touched(int f) { return f > 0; }\nbool Pad::held(int f) { return f < 0; }\nbool Pad::both(int f) { return touched(f) && held(f); }\n";
    let call = "#include \"pad.hpp\"\nbool any(Pad p) { return touched(1) || p.held(2); }\n";
    let twin = "bool touched(int f) { return false; }\n";
    let files = [
        ("src/pad.hpp", h),
        ("src/linux/pad.cpp", def),
        ("src/use.cpp", call),
    ];
    assert_eq!(
        cpp_targets(&files, "src/use.cpp", "any"),
        [
            "touched(1) → src/linux/pad.cpp:2",
            "p.held(2) → src/linux/pad.cpp:3"
        ]
    );
    let files = [files[0], files[1], files[2], ("src/mac/pad.mm", twin)];
    assert_eq!(
        cpp_targets(&files, "src/use.cpp", "any"),
        [
            "touched(1) → one of: src/linux/pad.cpp:2, or outside",
            "p.held(2) → one of: src/linux/pad.cpp:3, or outside"
        ]
    );
    // A call in the defining file reaches its own unit's definition.
    assert_eq!(
        cpp_targets(&files, "src/linux/pad.cpp", "both"),
        [
            "touched(f) → src/linux/pad.cpp:2",
            "held(f) → src/linux/pad.cpp:3"
        ]
    );
}

#[test]
fn a_declaration_whose_header_has_an_unread_cuda_twin_is_open() {
    let h = "void launch(int n);\n";
    let def = "#include \"../kern.h\"\nvoid launch(int n) {}\n";
    let call = "#include \"kern.h\"\nvoid go() { launch(4); }\n";
    let files = [
        ("src/kern.h", h),
        ("src/cpu/kern.cpp", def),
        ("src/use.cpp", call),
        ("src/gpu/kern.cu", "void launch(int n) {}\n"),
    ];
    assert_eq!(
        cpp_targets(&files, "src/use.cpp", "go"),
        ["launch(4) → one of: src/cpu/kern.cpp:2, or outside"]
    );
}

#[test]
fn a_parameter_of_a_callable_project_class_called_bare_is_its_call_operator() {
    let src = "struct Less {\n    bool operator()(int a, int b) const { return a < b; }\n};\nbool pick(Less& cmp, int a, int b) { return cmp(a, b); }\n";
    assert_eq!(
        cpp_targets(&[("src/a.cpp", src)], "src/a.cpp", "pick"),
        ["cmp(a, b) → src/a.cpp:2"]
    );
}

#[test]
fn a_bare_call_of_an_alias_is_a_construction_of_its_type_wherever_the_caller_stands() {
    let h = "#include <vector>\nnamespace geo {\ntemplate <typename T>\nstruct Duo {\n    T x, y;\n    Duo(T a, T b) : x(a), y(b) {}\n};\nusing DuoF = Duo<float>;\ntypedef Duo<int> DuoI;\ntemplate <typename T>\nusing Twin = Duo<T>;\n}\ntypedef std::vector<int> Ints;\n";
    let src = "#include \"vec.h\"\n#include <string>\nusing Str = std::string;\nstruct Outer : std::vector<int> {\n    void m();\n};\nvoid f() { geo::DuoF(1, 2); Str(\"x\"); Ints(3); }\nvoid Outer::m() { using namespace geo; DuoF(1, 2); DuoI(1, 2); Twin<float>(1, 2); Str(\"x\"); }\nvoid Loose::h() { using namespace geo; DuoF(1, 2); Ints(3); }\n";
    // Another file's alias of the name is out of this file's reach.
    let other = "#include \"vec.h\"\nusing Str = geo::Duo<char>;\n";
    let files = [("src/vec.h", h), ("src/a.cpp", src), ("src/b.cpp", other)];
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "f"),
        [
            "geo::DuoF(1, 2) → hidden",
            "Str(\"x\") → outside",
            "Ints(3) → outside"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "m"),
        [
            "DuoF(1, 2) → hidden",
            "DuoI(1, 2) → hidden",
            "Twin<float>(1, 2) → hidden",
            "Str(\"x\") → outside"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/a.cpp", "h"),
        ["DuoF(1, 2) → hidden", "Ints(3) → outside"]
    );
}

#[test]
fn a_bare_call_of_an_alias_reads_outside_only_on_evidence_its_type_is_from_outside() {
    let h = "template <class T>\nstruct Box {\n    using value_type = T;\n    value_type zero() { return value_type(0); }\n};\ntemplate <class A>\nstruct Holder {\n    using size_type = typename A::size_type;\n    size_type count() { return size_type(1); }\n};\nstruct Buf {\n    Buf(int n);\n};\n";
    let other = "#include <vector>\nnamespace other {\nusing Buf = std::vector<char>;\n}\n";
    let src = "#include \"box.h\"\n#include \"other.h\"\nstruct Walk : std::vector<int> {\n    void go();\n};\nvoid Walk::go() { Buf(3); }\n";
    let files = [
        ("src/box.h", h),
        ("src/other.h", other),
        ("src/use.cpp", src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/box.h", "zero"),
        ["value_type(0) → hidden"]
    );
    assert_eq!(
        cpp_targets(&files, "src/box.h", "count"),
        ["size_type(1) → hidden"]
    );
    // A class of the name in reach beats a namesake alias of an outside type.
    assert_eq!(
        cpp_targets(&files, "src/use.cpp", "go"),
        ["Buf(3) → hidden"]
    );
}

#[test]
fn a_function_with_a_macro_before_its_return_type_is_a_function() {
    // A long return type name (`tap_outcome_kind`) is what makes the
    // grammar read `TAP_API tap_outcome_kind tap_open` as `tap_outcome_kind::tap_open`.
    let h = "#define TAP_API extern\ntypedef int tap_outcome_kind;\ntypedef struct tap_dev tap_dev;\nTAP_API tap_outcome_kind tap_open(const tap_dev* d);\n";
    let src = "#include \"tap.h\"\nTAP_API tap_outcome_kind tap_open(const tap_dev* d)\n{\n    return d == 0;\n}";
    let call =
        "#include <tap.h>\nint use(const tap_dev* d) { return tap_open(d) + tap_shut(d); }\n";
    // Neither word tells which is the macro: no stronger than open.
    let other = "tap_layout tap_outcome_kind tap_shut(const tap_dev* d)\n{\n    return 0;\n}\n";
    let near = "#include \"../lib/tap.h\"\nint near(const tap_dev* d) { return tap_open(d); }\n";
    let files = [
        ("lib/tap.h", h),
        ("lib/tap.c", src),
        ("app/use.c", call),
        ("lib/shut.c", other),
        ("app/near.c", near),
    ];
    assert_eq!(
        cpp_targets(&files, "app/near.c", "near"),
        ["tap_open(d) → lib/tap.c:2"]
    );
    assert_eq!(
        cpp_targets(&files, "app/use.c", "use"),
        [
            "tap_open(d) → one of: lib/tap.c:2, or outside",
            "tap_shut(d) → one of: lib/shut.c:1, or outside"
        ]
    );
}

#[test]
fn a_method_with_a_macro_before_its_return_type_is_its_classs() {
    let h = "#define RUN_API\ntypedef unsigned int knob_u32;\ntypedef int outcome_kind_long;\nstruct Kern {\n    knob_u32 step(int a);\n    knob_u32 other_one(int a);\n};\nstruct Runner {\n    RUN_API outcome_kind_long run();\n    outcome_kind_long run_it();\n    outcome_kind_long run_short();\n};\n";
    let src = "#include \"kern.h\"\nknob_u32 Kern::other_one(int a) { return a; }\nFORCE_INLINE knob_u32 Kern::step(int a) { return other_one(a) + this->other_one(a); }\noutcome_kind_long Runner::run_short() { return 0; }\nRUN_API outcome_kind_long Runner::run_it() { return run_short(); }\nRUN_API outcome_kind_long Runner::run() { return 1; }\nint use(Kern k, Runner r) { return k.step(1) + r.run(); }\n";
    let files = [("src/kern.h", h), ("src/kern.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/kern.cpp", "step"),
        [
            "other_one(a) → src/kern.cpp:2",
            "this->other_one(a) → src/kern.cpp:2"
        ]
    );
    assert_eq!(
        cpp_targets(&files, "src/kern.cpp", "run_it"),
        ["run_short() → src/kern.cpp:4"]
    );
    assert_eq!(
        cpp_targets(&files, "src/kern.cpp", "use"),
        ["k.step(1) → src/kern.cpp:3", "r.run() → src/kern.cpp:6"]
    );
}

#[test]
fn a_misread_methods_bare_call_is_never_certain_to_a_free_namesake() {
    // Neither word is told the macro: the method stays misread, with no class.
    let h = "typedef int outcome_kind;\ntypedef int outcome_kind_long;\nstruct Runner {\n    outcome_kind_long run_it();\n    outcome_kind_long run_short();\n};\noutcome_kind_long run_short();\n";
    let src = "#include \"run.h\"\noutcome_kind_long Runner::run_short() { return 0; }\noutcome_kind_long run_short() { return 1; }\noutcome_kind outcome_kind_long Runner::run_it() { return run_short(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/run.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "run_it"
        ),
        ["run_short() → one of: src/run.cpp:2, src/run.cpp:3, or outside"]
    );
}

#[test]
fn a_misread_call_operators_bare_call_is_never_certain_to_a_free_namesake() {
    let h = "typedef int outcome_kind;\ntypedef int outcome_kind_long;\nstruct Runner {\n    outcome_kind_long operator()(int a);\n    outcome_kind_long run_short();\n};\noutcome_kind_long run_short();\n";
    let src = "#include \"run.h\"\noutcome_kind_long Runner::run_short() { return 0; }\noutcome_kind_long run_short() { return 1; }\noutcome_kind outcome_kind_long Runner::operator()(int a) { return run_short(); }\n";
    assert_eq!(
        cpp_targets(
            &[("src/run.h", h), ("src/run.cpp", src)],
            "src/run.cpp",
            "operator()"
        ),
        ["run_short() → one of: src/run.cpp:2, src/run.cpp:3, or outside"]
    );
}

#[test]
fn an_inline_conversion_operator_the_grammar_misreads_is_its_classs_and_defines_its_declaration() {
    // `inline K::operator bool()` parses as a function `bool` of the type
    // `K::operator`.
    let h = "namespace rig {\nstruct Ent {\n    explicit operator bool() const;\n    int stat() const;\n};\ninline Ent::operator bool() const\n{\n    stat();\n    return true;\n}\nstruct Deck {\n    struct Ent {\n        operator int() const;\n        int stat() const { return 2; }\n    };\n};\ninline Deck::Ent::operator int() const { return stat(); }\n}\n";
    let src =
        "#include \"ent.h\"\nint rig::Ent::stat() const { return 1; }\nint stat() { return 0; }\n";
    let files = [("src/ent.h", h), ("src/ent.cpp", src)];
    assert_eq!(
        cpp_targets(&files, "src/ent.h", "operator bool"),
        ["stat() → src/ent.cpp:2"]
    );
    assert_eq!(
        cpp_targets(&files, "src/ent.h", "operator int"),
        ["stat() → src/ent.h:14"]
    );
    let (_d, vault) = common::built(&files);
    let page = common::map_page(&vault, "map/src/ent_h");
    assert_eq!(
        headings(&page),
        [
            "ent_operator_bool",
            "ent_stat",
            "ent_operator_bool_2",
            "deck_ent_operator_int",
            "deck_ent_stat",
            "ent_operator_int"
        ]
    );
    assert_eq!(
        common::fn_block(&page, "ent_operator_bool"),
        "## ent_operator_bool\n`explicit operator bool() const` · `src/ent.h:3-3`\n\nDefined at [[map/src/ent_h#ent_operator_bool_2|Ent::operator bool]]\n\n"
    );
}

#[test]
fn a_call_of_a_function_template_with_an_explicit_specialization_is_never_certain() {
    let h = "struct Wide {};\nnamespace pk {\ntemplate <typename T> int pack(T v) { return 0; }\ntemplate <> int pack<Wide>(Wide v);\n}\nnamespace tray {\ntemplate <typename T> int pack(T v) { return 2; }\n}\n";
    let def = "#include \"pack.h\"\nnamespace pk {\ntemplate <> int pack<Wide>(Wide v) { return 1; }\n}\n";
    let src = "#include \"pack.h\"\nint send() { return pk::pack(Wide{}) + tray::pack(2); }\n";
    let files = [
        ("src/pack.h", h),
        ("src/pack.cpp", def),
        ("src/send.cpp", src),
    ];
    assert_eq!(
        cpp_targets(&files, "src/send.cpp", "send"),
        [
            "pk::pack(Wide{}) → one of: src/pack.h:3, or outside",
            "tray::pack(2) → src/pack.h:7"
        ]
    );
}

#[test]
fn a_bare_call_a_using_declaration_of_an_outside_function_reaches_may_be_outside() {
    // A project `namespace std` block is still no project function's.
    let h = "#include <utility>\nnamespace std {\ntemplate <> struct hash<int>;\n}\nnamespace geo {\nstruct Pt { int x; };\nvoid swap(Pt& a, Pt& b);\ntemplate <typename T>\nvoid swap(T* a, T* b) {}\ntemplate <typename T>\nvoid reorder(T& a, T& b)\n{\n    using std::swap;\n    swap(a, b);\n}\n}\n";
    let def = "#include \"geo.h\"\nnamespace geo {\nvoid swap(Pt& a, Pt& b) {}\n}\n";
    let at_file = "#include <other_lib.h>\n#include \"geo.h\"\nnamespace ol = other_lib;\nusing ol::swap;\nusing namespace geo;\nvoid show(Pt& a, Pt& b) { swap(a, b); }\n";
    let own =
        "#include \"geo.h\"\nusing geo::swap;\nvoid mine(geo::Pt& a, geo::Pt& b) { swap(a, b); }\n";
    let files = [
        ("src/geo.h", h),
        ("src/geo.cpp", def),
        ("src/show.cpp", at_file),
        ("src/mine.cpp", own),
    ];
    assert_eq!(
        cpp_targets(&files, "src/geo.h", "reorder"),
        ["swap(a, b) → one of: src/geo.cpp:3, src/geo.h:8, or outside"]
    );
    assert_eq!(
        cpp_targets(&files, "src/show.cpp", "show"),
        ["swap(a, b) → one of: src/geo.cpp:3, src/geo.h:8, or outside"]
    );
    assert_eq!(
        cpp_targets(&files, "src/mine.cpp", "mine"),
        ["swap(a, b) → one of: src/geo.cpp:3, src/geo.h:8"]
    );
}
