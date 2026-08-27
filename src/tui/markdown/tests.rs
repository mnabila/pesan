use super::*;

fn text_of(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn bold_sets_modifier_and_drops_syntax() {
    let lines = render("hello **world**", 80, &Theme::dark(), false);
    let joined = text_of(&lines);
    assert!(joined.contains("hello world"), "text: {joined:?}");
    assert!(!joined.contains("**"), "syntax leaked: {joined:?}");
    // The "world" span carries the BOLD modifier.
    let bold = lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content.contains("world") && s.style.add_modifier.contains(Modifier::BOLD));
    assert!(bold, "world not bold");
}

#[test]
fn heading_is_styled() {
    let lines = render("# Title", 80, &Theme::dark(), false);
    let styled = lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content.contains("Title") && s.style.add_modifier.contains(Modifier::BOLD));
    assert!(styled, "heading not bold: {:?}", text_of(&lines));
}

#[test]
fn bullet_list_renders_markers() {
    let lines = render("- one\n- two", 80, &Theme::dark(), false);
    let joined = text_of(&lines);
    assert!(joined.contains("• one"), "no bullet: {joined:?}");
    assert!(joined.contains("• two"), "no bullet: {joined:?}");
}

#[test]
fn ordered_list_numbers() {
    let lines = render("1. first\n2. second", 80, &Theme::dark(), false);
    let joined = text_of(&lines);
    assert!(joined.contains("1. first"), "{joined:?}");
    assert!(joined.contains("2. second"), "{joined:?}");
}

#[test]
fn link_shows_text_and_url() {
    let lines = render("[docs](https://acme.io/x)", 80, &Theme::dark(), false);
    let joined = text_of(&lines);
    assert!(joined.contains("docs"), "{joined:?}");
    assert!(joined.contains("https://acme.io/x"), "{joined:?}");
}

#[test]
fn autolink_omits_redundant_url() {
    let lines = render("<https://acme.io/x>", 80, &Theme::dark(), false);
    let joined = text_of(&lines);
    // The URL should appear once, not doubled as "url (url)".
    assert_eq!(joined.matches("acme.io/x").count(), 1, "{joined:?}");
}

#[test]
fn table_renders_rows() {
    let md = "| a | b |\n|---|---|\n| 1 | 2 |";
    let lines = render(md, 80, &Theme::dark(), false);
    let joined = text_of(&lines);
    assert!(joined.contains('a') && joined.contains('b'), "{joined:?}");
    assert!(joined.contains('1') && joined.contains('2'), "{joined:?}");
    assert!(joined.contains('│'), "no separators: {joined:?}");
}

#[test]
fn wraps_to_width() {
    let lines = render("aaaa bbbb cccc dddd", 9, &Theme::dark(), false);
    assert!(lines.len() > 1, "did not wrap: {:?}", text_of(&lines));
    for l in &lines {
        let w: usize = l.spans.iter().map(|s| s.content.width()).sum();
        assert!(w <= 9, "line too wide ({w}): {:?}", l);
    }
}

#[test]
fn code_block_preserved() {
    let md = "```\nlet x = 1;\n```";
    let lines = render(md, 80, &Theme::dark(), false);
    let joined = text_of(&lines);
    assert!(joined.contains("let x = 1;"), "{joined:?}");
}
