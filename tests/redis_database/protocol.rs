use super::*;

#[test]
fn parser_is_binary_safe_and_rejects_incomplete_input() {
    assert_eq!(
        parse_command(r#"SET 'a b' "中\x00\xFF\n\t\"\\""#).unwrap(),
        vec![
            b"SET".to_vec(),
            b"a b".to_vec(),
            ["中".as_bytes(), b"\0\xff\n\t\"\\"].concat()
        ]
    );
    assert_eq!(
        parse_command("SET a ''").unwrap(),
        vec![b"SET".to_vec(), b"a".to_vec(), vec![]]
    );
    assert_eq!(
        parse_command(r#"SET ab"cd" e\ f"#).unwrap()[1..],
        [b"abcd".to_vec(), b"e f".to_vec()]
    );
    for input in [
        "",
        "  ",
        "''",
        "GET 'open",
        "GET \"open",
        "GET a\\",
        r"GET \x0",
        r"GET \xGG",
        r"GET \q",
    ] {
        assert!(parse_command(input).is_err(), "{input}");
    }
    assert!(!command_needs_confirmation(
        &parse_command("gEt a").unwrap()
    ));
    assert!(command_needs_confirmation(
        &parse_command("SET a b").unwrap()
    ));
    assert!(command_needs_confirmation(
        &parse_command("EVAL script 0").unwrap()
    ));
    assert!(command_needs_confirmation(&[]));
    assert_eq!(display_bytes("中文".as_bytes()), "中文");
    assert_eq!(display_bytes(b"a\0\xff"), r"\x61\x00\xFF");
    for bytes in [vec![], vec![0, 255, 127], "中文\n".as_bytes().to_vec()] {
        assert_eq!(parse_hex(&format_hex(&bytes)).unwrap(), bytes);
    }
    assert_eq!(parse_hex("00 FF\n7f").unwrap(), [0, 255, 127]);
    for input in ["0", "0x12", "GG", "FF f"] {
        assert!(parse_hex(input).is_err());
    }
}

#[test]
fn json_formatting_preserves_arbitrary_precision_numbers() {
    let input = r#"{"id":123456789012345678901234567890,"value":0.12345678901234567890123456789}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let formatted = serde_json::to_string_pretty(&value).unwrap();
    assert!(formatted.contains("123456789012345678901234567890"));
    assert!(formatted.contains("0.12345678901234567890123456789"));
}
