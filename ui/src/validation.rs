//! 官方设置表单的纯前端校验规则。
//! host 与 wasm 共用，且不依赖 GPUI，便于直接单测。

/// 官方：自定义服务端点必须是可解析的 HTTP/HTTPS URL，
/// localhost、IP 字面量、端口与路径都有效；语法错误在字段处阻止创建。
pub(crate) fn valid_provider_url(value: &str) -> bool {
    let value = value.trim();
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    let Some(rest) = rest else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if let Some(port) = port
        && (port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    if host.starts_with('[') {
        return host.ends_with(']');
    }
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recurrence {
    Cron,
    RRule,
}

/// 官方：自定义规则由一个输入框接受五字段 Cron，或 DTSTART 与 RRULE 各一行。
/// 界面不向用户提示语法名称，因此错误文案保持中性。
pub(crate) fn validate_recurrence(value: &str) -> Result<Recurrence, &'static str> {
    let lines: Vec<&str> = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() == 1 {
        let fields: Vec<&str> = lines[0].split_whitespace().collect();
        let valid = fields.len() == 5
            && fields.iter().all(|field| {
                field.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(byte, b'*' | b',' | b'-' | b'/' | b'?' | b'#' | b'L' | b'W')
                })
            });
        return valid
            .then_some(Recurrence::Cron)
            .ok_or("运行规则格式无效，请检查后重试。");
    }
    if lines.len() == 2 {
        let first = lines[0].to_ascii_uppercase();
        let second = lines[1].to_ascii_uppercase();
        if (first.starts_with("DTSTART") && second.starts_with("RRULE"))
            || (first.starts_with("RRULE") && second.starts_with("DTSTART"))
        {
            return Ok(Recurrence::RRule);
        }
    }
    Err("运行规则格式无效，请检查后重试。")
}

#[cfg(test)]
mod tests {
    use super::{Recurrence, valid_provider_url, validate_recurrence};

    #[test]
    fn provider_urls_require_http_and_a_valid_host() {
        for value in [
            "https://api.deepseek.com",
            "https://api.deepseek.com/v1",
            "http://localhost:3888/v1",
            "http://127.0.0.1:8080",
            "https://my_host.example:443",
            "http://[::1]:8080",
        ] {
            assert!(valid_provider_url(value), "should accept {value}");
        }
        for value in [
            "",
            "api.deepseek.com",
            "ftp://api.deepseek.com",
            "https://",
            "https://host:port",
            "https://user@host",
            "https://host with space/v1",
        ] {
            assert!(!valid_provider_url(value), "should reject {value}");
        }
    }

    #[test]
    fn recurrence_accepts_five_field_cron_or_two_line_rrule() {
        assert_eq!(validate_recurrence("0 9 * * 1-5"), Ok(Recurrence::Cron));
        assert_eq!(validate_recurrence("*/15 * * * *"), Ok(Recurrence::Cron));
        assert_eq!(
            validate_recurrence("DTSTART:20260101T090000\nRRULE:FREQ=DAILY;COUNT=5"),
            Ok(Recurrence::RRule)
        );
        assert!(validate_recurrence("0 9 * *").is_err());
        assert!(validate_recurrence("RRULE:FREQ=DAILY").is_err());
        assert!(validate_recurrence("").is_err());
    }
}
