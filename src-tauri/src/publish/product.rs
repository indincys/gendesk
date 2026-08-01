//! 商品域的纯校验。

use std::collections::HashSet;

use crate::error::{AppError, AppResult};
use crate::publish::platform::Platform;

pub fn validate_code(code: &str) -> AppResult<String> {
    let code = code.trim();
    let valid = !code.is_empty()
        && code.len() <= 24
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !valid {
        return Err(AppError::InvalidInput(
            "商品短码只能包含字母、数字、-、_、.，且不超过 24 个字符".into(),
        ));
    }
    Ok(code.to_ascii_uppercase())
}

pub fn validate_platforms(platforms: &[String]) -> AppResult<()> {
    if platforms.is_empty() {
        return Err(AppError::InvalidInput("商品至少参与一个平台".into()));
    }
    let mut seen = HashSet::new();
    for code in platforms {
        if Platform::from_code(code).is_none() {
            return Err(AppError::InvalidInput(format!("未知平台：{code}")));
        }
        if !seen.insert(code) {
            return Err(AppError::InvalidInput(format!("平台重复：{code}")));
        }
    }
    Ok(())
}

pub fn validate_short_title(value: &str) -> AppResult<()> {
    if value.chars().count() > 10 {
        return Err(AppError::InvalidInput(
            "抖音挂车短标题不能超过 10 个字".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;

    #[test]
    fn product_code_is_ascii_and_normalized() {
        assert_eq!(validate_code(" a-1 ").unwrap(), "A-1");
        assert!(validate_code("商品A").is_err());
        assert!(validate_code("a b").is_err());
    }

    #[test]
    fn short_title_counts_unicode_scalars() {
        assert!(validate_short_title("一二三四五六七八九十").is_ok());
        assert!(validate_short_title("一二三四五六七八九十一").is_err());
    }

    #[test]
    fn platforms_must_be_known_unique_and_nonempty() {
        assert!(validate_platforms(&[]).is_err());
        assert!(validate_platforms(&["douyin".into(), "douyin".into()]).is_err());
        assert!(validate_platforms(&["douyin".into(), "xhs".into()]).is_ok());
    }
}
