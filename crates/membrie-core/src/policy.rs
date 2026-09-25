use crate::model::{CaptureCandidate, CaptureRule};

pub const MAX_AUTOMATIC_CONTENT_BYTES: usize = 256 * 1024;

pub fn sensitive_reason(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();

    if lower.contains("-----begin ") && lower.contains("private key-----") {
        return Some("private key material");
    }

    for marker in [
        "authorization: bearer ",
        "password=",
        "password:",
        "passwd=",
        "api_key=",
        "api-key:",
        "apikey=",
        "client_secret=",
        "access_token=",
        "refresh_token=",
    ] {
        if lower.contains(marker) {
            return Some("credential-like content");
        }
    }

    for token in text.split(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
            )
    }) {
        let token = token.trim_matches(|character: char| matches!(character, ':' | '='));
        if looks_like_known_token(token) {
            return Some("access token");
        }
        if looks_like_jwt(token) {
            return Some("JSON web token");
        }
    }

    if contains_payment_card_number(text) {
        return Some("payment-card number");
    }

    None
}

pub fn exclusion_reason<'a>(
    candidate: &CaptureCandidate,
    rules: &'a [CaptureRule],
) -> Option<&'a CaptureRule> {
    rules.iter().find(|rule| {
        if !rule.enabled {
            return false;
        }
        match rule.rule_type.as_str() {
            "app" => candidate
                .source_app
                .as_deref()
                .is_some_and(|value| wildcard_match(&rule.pattern, value)),
            "window_title" => candidate
                .window_title
                .as_deref()
                .is_some_and(|value| wildcard_match(&rule.pattern, value)),
            "domain" => candidate
                .source_uri
                .as_deref()
                .is_some_and(|value| wildcard_match(&rule.pattern, value)),
            "content" => wildcard_match(&rule.pattern, &candidate.body),
            _ => false,
        }
    })
}

pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let value: Vec<char> = value.to_lowercase().chars().collect();
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut star_value_index) = (None, 0);

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn looks_like_known_token(token: &str) -> bool {
    let prefixes = [
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "npm_",
        "pypi-AgEIcHlwaS",
        "hf_",
        "ya29.",
        "AIza",
        "sk-proj-",
        "sk_live_",
        "rk_live_",
    ];
    if prefixes
        .iter()
        .any(|prefix| token.starts_with(prefix) && token.len() >= prefix.len() + 12)
    {
        return true;
    }

    if token.starts_with("sk-") && token.len() >= 24 {
        return true;
    }

    token.len() == 20
        && token.starts_with("AKIA")
        && token
            .chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
}

fn looks_like_jwt(token: &str) -> bool {
    let mut parts = token.split('.');
    let Some(header) = parts.next() else {
        return false;
    };
    let Some(payload) = parts.next() else {
        return false;
    };
    let Some(signature) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && header.starts_with("eyJ")
        && payload.len() >= 8
        && signature.len() >= 8
        && token.len() >= 32
}

fn contains_payment_card_number(text: &str) -> bool {
    let mut candidate = String::new();
    for character in text.chars().chain(std::iter::once('\0')) {
        if character.is_ascii_digit() {
            candidate.push(character);
        } else if matches!(character, ' ' | '-') && !candidate.is_empty() {
            continue;
        } else if !candidate.is_empty() {
            if (13..=19).contains(&candidate.len()) && passes_luhn(&candidate) {
                return true;
            }
            candidate.clear();
        }
    }
    false
}

fn passes_luhn(digits: &str) -> bool {
    let mut sum = 0;
    let mut double = false;
    for character in digits.chars().rev() {
        let Some(mut digit) = character.to_digit(10) else {
            return false;
        };
        if double {
            digit *= 2;
            if digit > 9 {
                digit -= 9;
            }
        }
        sum += digit;
        double = !double;
    }
    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_high_confidence_secrets() {
        assert_eq!(
            sensitive_reason("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"),
            Some("private key material")
        );
        assert_eq!(
            sensitive_reason("Authorization: Bearer definitely-a-secret"),
            Some("credential-like content")
        );
        assert_eq!(
            sensitive_reason("ghp_1234567890abcdefghijklmnop"),
            Some("access token")
        );
        assert_eq!(
            sensitive_reason("sk-ant-1234567890abcdefghijklmnop"),
            Some("access token")
        );
        assert_eq!(
            sensitive_reason("4111 1111 1111 1111"),
            Some("payment-card number")
        );
        assert_eq!(sensitive_reason("Buy milk on the way home"), None);
    }

    #[test]
    fn wildcard_rules_are_case_insensitive() {
        assert!(wildcard_match("*KeePass*", "org.keepassxc.KeePassXC"));
        assert!(wildcard_match("private.?", "PRIVATE.1"));
        assert!(!wildcard_match("Bitwarden", "Firefox"));
    }
}
