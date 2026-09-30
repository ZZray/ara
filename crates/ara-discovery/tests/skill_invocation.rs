use ara_discovery::{ParsedSkillInvocation, parse_skill_invocation};

#[test]
fn fixed_upstream_invocation_forms_and_exclusions() {
    let cases = [
        ("/skill:foo", Some(("foo", ""))),
        ("  /skill:foo focus on auth", Some(("foo", "focus on auth"))),
        ("/skill:", None),
        ("leading /skill:foo trailing", Some(("foo", "leading trailing"))),
        ("explain this\nthen use /skill:foo ", Some(("foo", "explain this\nthen use"))),
        ("/compact /skill:foo", None),
        ("/goal set /skill:foo", None),
        ("!cmd /skill:foo", None),
        ("!!cmd /skill:foo", None),
        ("$ cmd /skill:foo", None),
        ("$$ cmd /skill:foo", None),
        ("$\tcmd /skill:foo", None),
        ("$echo /skill:reviewer", Some(("reviewer", "$echo"))),
        ("${HOME}/bin /skill:foo", Some(("foo", "${HOME}/bin"))),
        ("https://example.com/skill:foo", None),
        ("see /skill:foo/bar", None),
        ("before /skill:bad/path then /skill:ok after", Some(("ok", "before /skill:bad/path then after"))),
        ("/skill:foo\tbar", Some(("foo\tbar", ""))),
        ("/skill:foo/bar", Some(("foo/bar", ""))),
        ("before\u{a0}/skill:foo\u{a0}after", Some(("foo", "before after"))),
        ("before\u{200b}/skill:foo after", None),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse_skill_invocation(input),
            expected.map(|(name, args)| ParsedSkillInvocation { name: name.into(), args: args.into() }),
            "{input:?}"
        );
    }
}
