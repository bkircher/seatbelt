use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
};

pub(crate) fn shell_words(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg.as_os_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &OsStr) -> String {
    let bytes = arg.as_bytes();

    if bytes.is_empty() {
        return "''".to_owned();
    }

    if bytes.iter().all(|byte| is_shell_safe_byte(*byte)) {
        return bytes.iter().map(|byte| char::from(*byte)).collect();
    }

    if let Some(text) = arg.to_str() {
        return shell_single_quote(text);
    }

    shell_ansi_c_quote(bytes)
}

fn shell_single_quote(text: &str) -> String {
    let mut quoted = String::from("'");
    for character in text.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

fn shell_ansi_c_quote(bytes: &[u8]) -> String {
    let mut quoted = String::from("$'");
    for byte in bytes {
        match *byte {
            b'\'' => quoted.push_str("\\'"),
            b'\\' => quoted.push_str("\\\\"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            b'\t' => quoted.push_str("\\t"),
            0x20..=0x7e => quoted.push(char::from(*byte)),
            _ => push_shell_octal_escape(&mut quoted, *byte),
        }
    }
    quoted.push('\'');
    quoted
}

fn push_shell_octal_escape(quoted: &mut String, byte: u8) {
    quoted.push('\\');
    quoted.push(char::from(b'0' + (byte >> 6)));
    quoted.push(char::from(b'0' + ((byte >> 3) & 0o7)));
    quoted.push(char::from(b'0' + (byte & 0o7)));
}

fn is_shell_safe_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'_'
            | b'@'
            | b'%'
            | b'+'
            | b'='
            | b':'
            | b','
            | b'.'
            | b'/'
            | b'-'
    )
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;
    use crate::test_support::os;

    #[test]
    fn shell_words_preserves_non_ascii_arguments() {
        let command = [os("echo"), os("café")];

        let actual = shell_words(&command);

        assert_eq!(actual, "echo 'café'");
    }

    #[test]
    fn shell_words_escapes_non_utf8_arguments_without_replacement() {
        let command = [
            os("echo"),
            OsString::from_vec(vec![b'f', b'o', b'o', 0xff, b'b', b'a', b'r']),
        ];

        let actual = shell_words(&command);

        assert_eq!(actual, "echo $'foo\\377bar'");
    }
}
