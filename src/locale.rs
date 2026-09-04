use serde::{Deserialize, Serialize};
use std::env;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    English,
    Chinese,
}

impl Language {
    pub fn detect() -> Self {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|key| env::var(key).ok().filter(|value| !value.is_empty()))
            .map_or(Self::English, |value| Self::from_locale_name(&value))
    }

    pub const fn next(self) -> Self {
        match self {
            Self::English => Self::Chinese,
            Self::Chinese => Self::English,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::English => "EN",
            Self::Chinese => "中文",
        }
    }

    pub fn secondary_limits_status(self, count: usize, visible: bool) -> String {
        match (self, visible, count) {
            (Self::English, false, 1) => "1 secondary limit hidden · s Show".to_owned(),
            (Self::English, false, count) => {
                format!("{count} secondary limits hidden · s Show")
            }
            (Self::English, true, 1) => "1 secondary limit shown · s Hide".to_owned(),
            (Self::English, true, count) => {
                format!("{count} secondary limits shown · s Hide")
            }
            (Self::Chinese, false, count) => {
                format!("已隐藏 {count} 个次要限额 · s 显示")
            }
            (Self::Chinese, true, count) => {
                format!("已显示 {count} 个次要限额 · s 隐藏")
            }
        }
    }

    pub const fn copy(self) -> UiCopy {
        match self {
            Self::English => UiCopy {
                list_footer_prefix: "  ↑↓/jk Select · Enter Details · r Refresh · t ",
                model_footer_prefix: "  ↑↓/jk Select · m Quotas · r Refresh · t ",
                detail_footer_prefix: "  Esc Back · r Refresh · t ",
                footer_language_separator: " · l ",
                footer_quit: " · q Quit",
                no_plans: "No coding plans found",
                no_plans_hint: "Sign in to Codex, configure Claude, or install OMP",
                quota: "Quota",
                quota_short: "Q",
                time: "Time",
                time_short: "T",
                reason: "Reason",
                action: "Action",
                history_collecting: "Collecting history",
                history_one_sample: "Collecting history · 1 sample",
                loading: "Loading",
                stale: "Cached",
                unavailable: "Unavailable · Enter for details",
            },
            Self::Chinese => UiCopy {
                list_footer_prefix: "  ↑↓/jk 选择 · Enter 详情 · r 刷新 · t ",
                model_footer_prefix: "  ↑↓/jk 选择 · m 额度 · r 刷新 · t ",
                detail_footer_prefix: "  Esc 返回 · r 刷新 · t ",
                footer_language_separator: " · l ",
                footer_quit: " · q 退出",
                no_plans: "未发现 Coding Plan",
                no_plans_hint: "登录 Codex、配置 Claude 状态行或安装 OMP",
                quota: "配额",
                quota_short: "额",
                time: "时间",
                time_short: "时",
                reason: "原因",
                action: "处理",
                history_collecting: "历史收集中",
                history_one_sample: "历史收集中 · 1 个样本",
                loading: "加载中",
                stale: "缓存",
                unavailable: "不可用 · Enter 查看原因",
            },
        }
    }

    fn from_locale_name(value: &str) -> Self {
        if value.trim().to_ascii_lowercase().starts_with("zh") {
            Self::Chinese
        } else {
            Self::English
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiCopy {
    pub list_footer_prefix: &'static str,
    pub model_footer_prefix: &'static str,
    pub detail_footer_prefix: &'static str,
    pub footer_language_separator: &'static str,
    pub footer_quit: &'static str,
    pub no_plans: &'static str,
    pub no_plans_hint: &'static str,
    pub quota: &'static str,
    pub quota_short: &'static str,
    pub time: &'static str,
    pub time_short: &'static str,
    pub reason: &'static str,
    pub action: &'static str,
    pub history_collecting: &'static str,
    pub history_one_sample: &'static str,
    pub loading: &'static str,
    pub stale: &'static str,
    pub unavailable: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_names_select_chinese_only_for_zh_locales() {
        assert_eq!(Language::from_locale_name("zh_CN.UTF-8"), Language::Chinese);
        assert_eq!(Language::from_locale_name("zh-TW"), Language::Chinese);
        assert_eq!(Language::from_locale_name("en_US.UTF-8"), Language::English);
        assert_eq!(Language::from_locale_name("C"), Language::English);
    }

    #[test]
    fn language_cycle_wraps() {
        assert_eq!(Language::English.next(), Language::Chinese);
        assert_eq!(Language::Chinese.next(), Language::English);
    }

    #[test]
    fn secondary_limit_status_is_localized_for_visibility_and_count() {
        assert_eq!(
            Language::English.secondary_limits_status(1, false),
            "1 secondary limit hidden · s Show"
        );
        assert_eq!(
            Language::English.secondary_limits_status(2, true),
            "2 secondary limits shown · s Hide"
        );
        assert_eq!(
            Language::Chinese.secondary_limits_status(2, false),
            "已隐藏 2 个次要限额 · s 显示"
        );
        assert_eq!(
            Language::Chinese.secondary_limits_status(1, true),
            "已显示 1 个次要限额 · s 隐藏"
        );
    }
}
