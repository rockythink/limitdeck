use std::time::{Duration, SystemTime};

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    adapter::{AdapterError, AdapterErrorKind},
    app::{App, PlanPhase, PlanState},
    domain::{UsageStatus, UsageWindow},
    history::{HistorySample, UsageHistory},
    locale::{Language, UiCopy},
    theme::{provider_accent, Palette, Theme},
};

const FILLED_BAR_GLYPH: &str = "━";
const EMPTY_BAR_GLYPH: &str = "─";
const LIST_PERIOD_WIDTH: usize = 3;

pub fn render(frame: &mut Frame<'_>, app: &App, history: &UsageHistory) {
    let area = frame.area();
    let palette = app.theme().palette();
    let language = app.language();
    let copy = language.copy();
    frame.render_widget(
        Block::default().style(Style::default().bg(palette.background)),
        area,
    );

    let (content_area, footer_area) = if area.height >= 3 {
        (
            Rect::new(area.x, area.y, area.width, area.height - 1),
            Some(row(area, area.height - 1)),
        )
    } else {
        (area, None)
    };

    if app.is_detail_open() {
        render_detail(frame, content_area, app, history, palette, language);
    } else {
        render_plan_list(frame, content_area, app, palette, language);
    }

    if let Some(footer_area) = footer_area {
        let prefix = if app.is_detail_open() {
            copy.detail_footer_prefix
        } else {
            copy.list_footer_prefix
        };
        let spans = footer_spans(prefix, app.theme(), language, copy, palette);
        render_line(frame, footer_area, spans, palette.background);
    }
}

fn footer_spans(
    prefix: &'static str,
    theme: Theme,
    language: Language,
    copy: UiCopy,
    palette: Palette,
) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(13);
    spans.push(Span::styled(prefix, Style::default().fg(palette.muted)));
    if theme == Theme::Rainbow {
        for (letter, color) in [
            ("R", Color::Rgb(255, 91, 146)),
            ("a", Color::Rgb(255, 143, 96)),
            ("i", Color::Rgb(255, 202, 87)),
            ("n", Color::Rgb(80, 235, 190)),
            ("b", Color::Rgb(64, 214, 255)),
            ("o", Color::Rgb(91, 157, 255)),
            ("w", Color::Rgb(181, 117, 255)),
        ] {
            spans.push(Span::styled(
                letter,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        spans.push(Span::styled(
            theme.label(),
            Style::default()
                .fg(palette.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        copy.footer_language_separator,
        Style::default().fg(palette.muted),
    ));
    spans.push(Span::styled(
        language.label(),
        Style::default()
            .fg(palette.warning)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        copy.footer_quit,
        Style::default().fg(palette.muted),
    ));
    spans
}

fn render_plan_list(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    palette: Palette,
    language: Language,
) {
    if area.width < 12 || area.height == 0 {
        return;
    }
    let copy = language.copy();
    if app.plans().is_empty() {
        render_line(
            frame,
            row(area, 0),
            vec![Span::styled(
                copy.no_plans,
                Style::default().fg(palette.muted),
            )],
            palette.background,
        );
        if area.height > 1 {
            render_line(
                frame,
                row(area, 1),
                vec![Span::styled(
                    copy.no_plans_hint,
                    Style::default().fg(palette.muted),
                )],
                palette.background,
            );
        }
        return;
    }

    let viewport_height = usize::from(area.height);
    let start = viewport_start(app, area.width, viewport_height);
    let mut screen_row = 0usize;

    for (index, plan) in app.plans().iter().enumerate().skip(start) {
        let selected = index == app.selected_index();
        let background = if selected {
            palette.surface
        } else {
            palette.background
        };
        let rows = plan_rows(
            plan,
            area.width,
            viewport_height,
            selected,
            palette,
            language,
        );
        if screen_row + rows.len() > viewport_height {
            break;
        }
        for spans in rows {
            render_line(frame, row(area, screen_row as u16), spans, background);
            screen_row += 1;
        }
    }
}

fn viewport_start(app: &App, width: u16, viewport_height: usize) -> usize {
    let selected = app.selected_index();
    let mut start = 0usize;
    let mut occupied = app.plans()[..=selected]
        .iter()
        .map(|plan| plan_height(plan, width, viewport_height))
        .sum::<usize>();

    while occupied > viewport_height && start < selected {
        occupied =
            occupied.saturating_sub(plan_height(&app.plans()[start], width, viewport_height));
        start += 1;
    }
    start
}

fn plan_height(state: &PlanState, width: u16, viewport_height: usize) -> usize {
    let Some(plan) = &state.plan else {
        return 1;
    };
    if uses_stacked_rows(state, width, viewport_height) {
        plan.windows.len().saturating_mul(2)
    } else {
        2
    }
}

fn uses_stacked_rows(state: &PlanState, width: u16, viewport_height: usize) -> bool {
    let Some(plan) = &state.plan else {
        return false;
    };
    width >= 40 && plan.windows.len() > 2 && plan.windows.len().saturating_mul(2) <= viewport_height
}

fn plan_rows(
    state: &PlanState,
    width: u16,
    viewport_height: usize,
    selected: bool,
    palette: Palette,
    language: Language,
) -> Vec<Vec<Span<'static>>> {
    if uses_stacked_rows(state, width, viewport_height) {
        return stacked_plan_rows(state, width, selected, palette, language);
    }
    plan_comparison_rows(state, width, selected, palette, language)
}

fn stacked_plan_rows(
    state: &PlanState,
    width: u16,
    selected: bool,
    palette: Palette,
    language: Language,
) -> Vec<Vec<Span<'static>>> {
    let accent = provider_accent(&state.identity.provider_id, palette);
    let name_width = 9usize;
    let model_width = 7usize;
    let prefix_width = 2 + name_width;
    let copy = language.copy();
    let metric_label_width = copy.quota_short.width().max(copy.time_short.width());
    let bar_width =
        metric_bar_width(width, prefix_width + model_width, 1, metric_label_width).unwrap_or(1);
    let plan = state.plan.as_ref().expect("stacked rows require a plan");
    let mut rows = Vec::with_capacity(plan.windows.len().saturating_mul(2));

    for (index, window) in plan.windows.iter().enumerate() {
        let mut quota = if index == 0 {
            vec![
                Span::styled(
                    if selected { "› " } else { "  " },
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    fit_name(&state.identity.display_name, name_width),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
            ]
        } else {
            vec![Span::raw(" ".repeat(prefix_width))]
        };
        let row_accent = window_accent(window, accent, palette);
        quota.push(Span::styled(
            fit_name(&compact_window_model(window), model_width),
            Style::default().fg(row_accent),
        ));
        let period = period_label(window.period, language);
        quota.push(Span::styled(
            format!(" {} ", pad_left_display(&period, LIST_PERIOD_WIDTH)),
            Style::default().fg(palette.muted),
        ));
        let quota_percent =
            (window.status == UsageStatus::Available).then_some(window.remaining_percent);
        push_labeled_progress(
            &mut quota,
            copy.quota_short,
            metric_label_width,
            quota_percent,
            bar_width,
            [
                row_accent,
                quota_percent.map_or(palette.muted, |_| percent_color(window, palette)),
            ],
            palette,
        );
        let phase = phase_text(state.phase, language);
        if index == 0 && !phase.is_empty() && width >= 64 {
            quota.push(Span::styled(
                format!(" {phase}"),
                Style::default().fg(palette.muted),
            ));
        }
        rows.push(quota);

        let mut time = vec![Span::raw(" ".repeat(prefix_width + model_width))];
        time.push(Span::styled(
            format!(" {} ", pad_left_display(&period, LIST_PERIOD_WIDTH)),
            Style::default().fg(palette.muted),
        ));
        push_labeled_progress(
            &mut time,
            copy.time_short,
            metric_label_width,
            window.remaining_time_percent_at(SystemTime::now()),
            bar_width,
            [palette.time, palette.time],
            palette,
        );
        rows.push(time);
    }
    rows
}

fn plan_comparison_rows(
    state: &PlanState,
    width: u16,
    selected: bool,
    palette: Palette,
    language: Language,
) -> Vec<Vec<Span<'static>>> {
    let accent = provider_accent(&state.identity.provider_id, palette);
    let name_width = if width < 32 { 7 } else { 9 };
    let mut quota = vec![
        Span::styled(
            if selected { "› " } else { "  " },
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            fit_name(&state.identity.display_name, name_width),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ];

    let Some(plan) = &state.plan else {
        quota.push(Span::styled(
            phase_text(state.phase, language),
            Style::default().fg(palette.muted),
        ));
        return vec![quota];
    };

    let prefix_width = 2 + name_width;
    let mut time = vec![Span::raw(" ".repeat(prefix_width))];
    let copy = language.copy();
    let metric_label_width = copy.quota_short.width().max(copy.time_short.width());
    if let Some(bar_width) =
        metric_bar_width(width, prefix_width, plan.windows.len(), metric_label_width)
    {
        for window in &plan.windows {
            let period = period_label(window.period, language);
            let period_cell = format!(" {} ", pad_left_display(&period, LIST_PERIOD_WIDTH));
            quota.push(Span::styled(
                period_cell.clone(),
                Style::default().fg(palette.muted),
            ));
            time.push(Span::styled(
                period_cell,
                Style::default().fg(palette.muted),
            ));
            let quota_percent =
                (window.status == UsageStatus::Available).then_some(window.remaining_percent);
            push_labeled_progress(
                &mut quota,
                copy.quota_short,
                metric_label_width,
                quota_percent,
                bar_width,
                [
                    window_accent(window, accent, palette),
                    quota_percent.map_or(palette.muted, |_| percent_color(window, palette)),
                ],
                palette,
            );
            push_labeled_progress(
                &mut time,
                copy.time_short,
                metric_label_width,
                window.remaining_time_percent_at(SystemTime::now()),
                bar_width,
                [palette.time, palette.time],
                palette,
            );
        }
    } else {
        let (detailed_quota, detailed_time) =
            compact_metric_rows(&plan.windows, copy.quota_short, copy.time_short, true);
        let use_markers =
            prefix_width + detailed_quota.width().max(detailed_time.width()) <= usize::from(width);
        let (quota_values, time_values) = if use_markers {
            (detailed_quota, detailed_time)
        } else {
            compact_metric_rows(&plan.windows, copy.quota_short, copy.time_short, false)
        };
        quota.push(Span::styled(
            quota_values,
            Style::default().fg(palette.text),
        ));
        time.push(Span::styled(time_values, Style::default().fg(palette.time)));
    }

    let phase = phase_text(state.phase, language);
    if !phase.is_empty() && width >= 64 {
        quota.push(Span::styled(
            format!(" {phase}"),
            Style::default().fg(palette.muted),
        ));
    }
    vec![quota, time]
}

fn push_labeled_progress(
    spans: &mut Vec<Span<'static>>,
    label: &str,
    label_width: usize,
    percent: Option<u8>,
    bar_width: usize,
    colors: [Color; 2],
    palette: Palette,
) {
    let [accent, value_color] = colors;
    spans.push(Span::styled(
        format!(
            "{label}{} ",
            " ".repeat(label_width.saturating_sub(label.width()))
        ),
        Style::default().fg(palette.muted),
    ));
    let filled = percent.map_or(0, |value| (usize::from(value) * bar_width + 50) / 100);
    spans.push(Span::styled(
        FILLED_BAR_GLYPH.repeat(filled),
        Style::default().fg(accent),
    ));
    spans.push(Span::styled(
        EMPTY_BAR_GLYPH.repeat(bar_width - filled),
        Style::default().fg(palette.border),
    ));
    let value = percent.map_or_else(|| "--".to_owned(), |value| format!("{value}%"));
    spans.push(Span::styled(
        format!(" {value:>4}"),
        Style::default().fg(value_color),
    ));
}

fn metric_bar_width(
    width: u16,
    prefix_width: usize,
    windows: usize,
    label_width: usize,
) -> Option<usize> {
    const PERIOD_COLUMNS: usize = 5;
    const VALUE_COLUMNS: usize = 6;
    let fixed = prefix_width
        .checked_add(windows.checked_mul(PERIOD_COLUMNS + label_width + VALUE_COLUMNS)?)?;
    let available = usize::from(width).checked_sub(fixed)?;
    (available >= windows).then(|| (available / windows).clamp(1, 8))
}

fn compact_metric_rows(
    windows: &[UsageWindow],
    quota_label: &str,
    time_label: &str,
    markers: bool,
) -> (String, String) {
    let mut quota_cells = Vec::with_capacity(windows.len());
    let mut time_cells = Vec::with_capacity(windows.len());
    let mut widths = Vec::with_capacity(windows.len());
    for window in windows {
        let quota_percent =
            (window.status == UsageStatus::Available).then_some(window.remaining_percent);
        let time_percent = window.remaining_time_percent_at(SystemTime::now());
        let quota = if markers {
            format!(
                "{}{}",
                progress_marker(quota_percent),
                compact_progress_value(quota_percent)
            )
        } else {
            compact_progress_value(quota_percent)
        };
        let time = if markers {
            format!(
                "{}{}",
                progress_marker(time_percent),
                compact_progress_value(time_percent)
            )
        } else {
            compact_progress_value(time_percent)
        };
        widths.push(quota.width().max(time.width()));
        quota_cells.push(quota);
        time_cells.push(time);
    }
    let format_row = |label: &str, cells: Vec<String>| {
        let values = cells
            .into_iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect::<Vec<_>>()
            .join("/");
        format!("{label}{values}")
    };
    (
        format_row(quota_label, quota_cells),
        format_row(time_label, time_cells),
    )
}

fn compact_window_model(window: &UsageWindow) -> String {
    if is_spark_window(window) {
        return "Spark".to_owned();
    }
    let label = window
        .label
        .split_once('·')
        .map_or(window.label.as_str(), |(model, _)| model)
        .trim();
    if label.contains("Codex") {
        "Codex".to_owned()
    } else {
        label.to_owned()
    }
}

fn localized_window_label(window: &UsageWindow, language: Language) -> String {
    if window.id.ends_with(":spend-limit") {
        return match language {
            Language::English => "Claude · Spend limit".to_owned(),
            Language::Chinese => "Claude · 消费限额".to_owned(),
        };
    }
    let model = window
        .label
        .split_once('·')
        .map_or(window.label.as_str(), |(model, _)| model)
        .trim();
    format!("{model} · {}", long_period_label(window.period, language))
}

fn is_spark_window(window: &UsageWindow) -> bool {
    window
        .id
        .split(':')
        .any(|part| part.eq_ignore_ascii_case("spark"))
        || window
            .label
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|part| part.eq_ignore_ascii_case("spark"))
}

fn window_accent(window: &UsageWindow, default: Color, palette: Palette) -> Color {
    if !is_spark_window(window) {
        return default;
    }
    match window.period {
        Some(period) if period < Duration::from_secs(24 * 60 * 60) => palette.spark_short,
        _ => palette.spark_long,
    }
}

fn render_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    history: &UsageHistory,
    palette: Palette,
    language: Language,
) {
    if area.width < 12 || area.height == 0 {
        return;
    }
    let Some(state) = app.selected_plan() else {
        return;
    };
    let copy = language.copy();
    let accent = provider_accent(&state.identity.provider_id, palette);
    let freshness = state
        .plan
        .as_ref()
        .map(|plan| freshness(plan.fetched_at, language))
        .unwrap_or_else(|| phase_text(state.phase, language).to_owned());
    render_line(
        frame,
        row(area, 0),
        vec![
            Span::styled("‹ ", Style::default().fg(accent)),
            Span::styled(
                state.identity.display_name.clone(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {freshness}"), Style::default().fg(palette.muted)),
        ],
        palette.surface,
    );

    let mut next_y = 1;
    if let Some(error) = state.error {
        if next_y < area.height {
            let reason = format!(
                "{}  {} · {}",
                copy.reason,
                error.source,
                diagnostic_title(error.kind, language)
            );
            render_line(
                frame,
                row(area, next_y),
                vec![Span::styled(
                    fit_display_width(&reason, usize::from(area.width)),
                    Style::default().fg(palette.warning),
                )],
                palette.background,
            );
            next_y += 1;
        }
        if next_y < area.height {
            let action = format!("{}  {}", copy.action, diagnostic_action(error, language));
            render_line(
                frame,
                row(area, next_y),
                vec![Span::styled(
                    fit_display_width(&action, usize::from(area.width)),
                    Style::default().fg(palette.muted),
                )],
                palette.background,
            );
            next_y += 1;
        }
    }

    if let Some(plan) = &state.plan {
        let labels = plan
            .windows
            .iter()
            .map(|window| localized_window_label(window, language))
            .collect::<Vec<_>>();
        let metric_label_width = copy.quota.width().max(copy.time.width());
        let label_limit = usize::from(area.width).saturating_sub(metric_label_width + 12);
        let columns = DetailColumns {
            label: labels
                .iter()
                .map(|label| label.width())
                .max()
                .unwrap_or(0)
                .min(label_limit),
            reset: plan
                .windows
                .iter()
                .map(|window| reset_text(window.resets_at, language).width())
                .max()
                .unwrap_or(0),
        };
        let available_rows = usize::from(area.height.saturating_sub(next_y));
        let show_trends =
            area.width >= 40 && available_rows >= plan.windows.len().saturating_mul(3);
        for (window, label) in plan.windows.iter().zip(labels) {
            if next_y.saturating_add(1) >= area.height {
                break;
            }
            let window_accent = window_accent(window, accent, palette);
            let rows = detail_window_rows(
                window,
                &label,
                area.width,
                columns,
                window_accent,
                palette,
                language,
            );
            for spans in rows {
                render_line(frame, row(area, next_y), spans, palette.background);
                next_y += 1;
            }

            if show_trends && next_y < area.height {
                let samples = history.samples(&plan.provider_id, &plan.id, &window.id);
                render_line(
                    frame,
                    row(area, next_y),
                    trend_line(samples, area.width, window_accent, palette, language),
                    palette.background,
                );
                next_y += 1;
            }
        }
    } else if state.error.is_none() && next_y < area.height {
        render_line(
            frame,
            row(area, next_y),
            vec![Span::styled(
                phase_text(state.phase, language),
                Style::default().fg(palette.muted),
            )],
            palette.background,
        );
    }
}

fn diagnostic_title(kind: AdapterErrorKind, language: Language) -> &'static str {
    match (language, kind) {
        (Language::English, AdapterErrorKind::CommandNotFound) => "Source command not found",
        (Language::English, AdapterErrorKind::NotAuthenticated) => "Source is not signed in",
        (Language::English, AdapterErrorKind::TimedOut) => "Source timed out",
        (Language::English, AdapterErrorKind::ProtocolChanged) => "Source data format changed",
        (Language::English, AdapterErrorKind::SnapshotMissing) => "No usage snapshot yet",
        (Language::English, AdapterErrorKind::SnapshotExpired) => "Cached snapshot expired",
        (Language::Chinese, AdapterErrorKind::CommandNotFound) => "未找到来源命令",
        (Language::Chinese, AdapterErrorKind::NotAuthenticated) => "来源尚未登录",
        (Language::Chinese, AdapterErrorKind::TimedOut) => "来源响应超时",
        (Language::Chinese, AdapterErrorKind::ProtocolChanged) => "来源数据格式已变化",
        (Language::Chinese, AdapterErrorKind::SnapshotMissing) => "尚无用量快照",
        (Language::Chinese, AdapterErrorKind::SnapshotExpired) => "缓存快照已过期",
    }
}

fn diagnostic_action(error: AdapterError, language: Language) -> String {
    match (language, error.kind) {
        (Language::English, AdapterErrorKind::CommandNotFound) => format!(
            "Install {}, confirm it is on PATH, then press r to retry",
            error.source
        ),
        (Language::English, AdapterErrorKind::NotAuthenticated) => {
            format!("Sign in to {}, then press r to retry", error.source)
        }
        (Language::English, AdapterErrorKind::TimedOut) => {
            "Check the network connection, then press r to retry".to_owned()
        }
        (Language::English, AdapterErrorKind::ProtocolChanged) => {
            "Upgrade LimitDeck; open an issue if it still fails".to_owned()
        }
        (Language::English, AdapterErrorKind::SnapshotMissing)
            if error.source == "Claude statusline" =>
        {
            "Configure Claude statusLine to run limitdeck ingest claude, then press r to retry"
                .to_owned()
        }
        (Language::English, AdapterErrorKind::SnapshotMissing) => {
            format!(
                "Run {} to create a snapshot, then press r to retry",
                error.source
            )
        }
        (Language::English, AdapterErrorKind::SnapshotExpired) => {
            format!("Refresh {}, then press r to retry", error.source)
        }
        (Language::Chinese, AdapterErrorKind::CommandNotFound) => {
            format!("安装 {}，确认命令已加入 PATH，然后按 r 重试", error.source)
        }
        (Language::Chinese, AdapterErrorKind::NotAuthenticated) => {
            format!("登录 {}，然后按 r 重试", error.source)
        }
        (Language::Chinese, AdapterErrorKind::TimedOut) => "检查网络连接，然后按 r 重试".to_owned(),
        (Language::Chinese, AdapterErrorKind::ProtocolChanged) => {
            "升级 LimitDeck；若仍失败，请提交 issue".to_owned()
        }
        (Language::Chinese, AdapterErrorKind::SnapshotMissing)
            if error.source == "Claude statusline" =>
        {
            "配置 Claude statusLine 运行 limitdeck ingest claude，然后按 r 重试".to_owned()
        }
        (Language::Chinese, AdapterErrorKind::SnapshotMissing) => {
            format!("先运行 {} 生成快照，然后按 r 重试", error.source)
        }
        (Language::Chinese, AdapterErrorKind::SnapshotExpired) => {
            format!("刷新 {}，然后按 r 重试", error.source)
        }
    }
}

fn trend_line(
    samples: &[HistorySample],
    width: u16,
    accent: Color,
    palette: Palette,
    language: Language,
) -> Vec<Span<'static>> {
    let samples = current_cycle(samples);
    let copy = language.copy();
    let mut spans = vec![Span::raw("    ")];
    let Some(first) = samples.first() else {
        spans.push(Span::styled(
            copy.history_collecting,
            Style::default().fg(palette.muted),
        ));
        return spans;
    };
    if samples.len() == 1 {
        spans.push(Span::styled(
            copy.history_one_sample,
            Style::default().fg(palette.muted),
        ));
        return spans;
    }

    let last = samples
        .last()
        .expect("history contains at least two samples");
    let span = history_span(first.at_millis, last.at_millis, language);
    let delta = i16::from(last.remaining_percent) - i16::from(first.remaining_percent);
    if delta == 0 {
        let flat = match language {
            Language::English => "flat",
            Language::Chinese => "持平",
        };
        spans.push(Span::styled(
            format!(
                "{span}  {}% · {flat} · {}",
                last.remaining_percent,
                sample_count(samples.len(), language)
            ),
            Style::default().fg(palette.muted),
        ));
        return spans;
    }

    spans.push(Span::styled(
        format!("{span}  "),
        Style::default().fg(palette.muted),
    ));
    if samples.len() >= 3 && width >= 56 {
        let graph_width = usize::from(width.saturating_sub(48)).clamp(8, 16);
        spans.push(Span::styled(
            braille_chart(samples, graph_width),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(
                "  {}% → {}% · {}",
                first.remaining_percent,
                last.remaining_percent,
                delta_text(delta)
            ),
            Style::default().fg(palette.muted),
        ));
    } else {
        spans.push(Span::styled(
            format!(
                "{}% → {}% · {} · {}",
                first.remaining_percent,
                last.remaining_percent,
                delta_text(delta),
                sample_count(samples.len(), language)
            ),
            Style::default().fg(palette.muted),
        ));
    }
    spans
}

fn current_cycle(samples: &[HistorySample]) -> &[HistorySample] {
    let start = samples
        .windows(2)
        .rposition(|pair| pair[1].remaining_percent > pair[0].remaining_percent)
        .map_or(0, |index| index + 1);
    &samples[start..]
}

fn history_span(first_millis: u64, last_millis: u64, language: Language) -> String {
    let seconds = last_millis.saturating_sub(first_millis) / 1_000;
    let (value, unit) = if seconds < 60 {
        (seconds.max(1), "second")
    } else if seconds < 60 * 60 {
        (seconds / 60, "minute")
    } else if seconds < 24 * 60 * 60 {
        (seconds / (60 * 60), "hour")
    } else {
        (seconds / (24 * 60 * 60), "day")
    };
    match language {
        Language::English => format!("Last {}", count_with_unit(value, unit)),
        Language::Chinese => match unit {
            "second" => format!("近 {value} 秒"),
            "minute" => format!("近 {value} 分钟"),
            "hour" => format!("近 {value} 小时"),
            _ => format!("近 {value} 天"),
        },
    }
}

fn sample_count(count: usize, language: Language) -> String {
    match language {
        Language::English => count_with_unit(count as u64, "sample"),
        Language::Chinese => format!("{count} 个样本"),
    }
}

fn count_with_unit(value: u64, unit: &str) -> String {
    let suffix = if value == 1 { "" } else { "s" };
    format!("{value} {unit}{suffix}")
}

fn delta_text(delta: i16) -> String {
    if delta > 0 {
        format!("+{delta}%")
    } else {
        format!("−{}%", delta.unsigned_abs())
    }
}

fn braille_chart(samples: &[HistorySample], width: usize) -> String {
    const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

    let min = samples
        .iter()
        .map(|sample| sample.remaining_percent)
        .min()
        .expect("Braille chart requires samples");
    let max = samples
        .iter()
        .map(|sample| sample.remaining_percent)
        .max()
        .expect("Braille chart requires samples");
    let range = u32::from(max - min).max(1);
    let pixel_columns = width.saturating_mul(2).max(2);
    let position_denominator = pixel_columns - 1;
    let sample_denominator = samples.len() - 1;
    let mut graph = String::with_capacity(width * 3);

    for cell in 0..width {
        let mut dots = 0_u8;
        for (column, dot_rows) in DOTS.iter().enumerate() {
            let x = cell * 2 + column;
            let position = x * sample_denominator;
            let lower = position / position_denominator;
            let remainder = position % position_denominator;
            let upper = (lower + 1).min(sample_denominator);
            let lower_value = usize::from(samples[lower].remaining_percent);
            let upper_value = usize::from(samples[upper].remaining_percent);
            let interpolated = (lower_value * (position_denominator - remainder)
                + upper_value * remainder)
                / position_denominator;
            let vertical = (u32::from(max) - interpolated as u32) * 3;
            let row = ((vertical + range / 2) / range).min(3) as usize;
            dots |= dot_rows[row];
        }
        graph.push(char::from_u32(0x2800 + u32::from(dots)).expect("valid Braille cell"));
    }
    graph
}

#[derive(Clone, Copy)]
struct DetailColumns {
    label: usize,
    reset: usize,
}

fn detail_window_rows(
    window: &UsageWindow,
    localized_label: &str,
    width: u16,
    columns: DetailColumns,
    accent: Color,
    palette: Palette,
    language: Language,
) -> [Vec<Span<'static>>; 2] {
    let label = fit_display_width(localized_label, columns.label);
    let label_padding = columns.label.saturating_sub(label.width());
    let reset = reset_text(window.resets_at, language);
    let copy = language.copy();
    let total_width = usize::from(width);
    let metric_label_width = copy.quota.width().max(copy.time.width());
    let prefix_width = 2 + columns.label + 2;
    let metric_fixed_width = metric_label_width + 6;
    let reset_columns = 2 + columns.reset;
    let show_reset =
        width >= 48 && total_width >= prefix_width + metric_fixed_width + reset_columns + 2;
    let reserved_width =
        prefix_width + metric_fixed_width + usize::from(show_reset) * reset_columns + 1;
    let bar_width = total_width.saturating_sub(reserved_width).clamp(1, 20);

    let mut quota = vec![Span::styled(
        format!("  {label}{}  ", " ".repeat(label_padding)),
        Style::default().fg(palette.text),
    )];
    let quota_percent =
        (window.status == UsageStatus::Available).then_some(window.remaining_percent);
    push_labeled_progress(
        &mut quota,
        copy.quota,
        metric_label_width,
        quota_percent,
        bar_width,
        [
            accent,
            quota_percent.map_or(palette.muted, |_| percent_color(window, palette)),
        ],
        palette,
    );
    if show_reset {
        quota.push(Span::styled(
            format!("  {reset}"),
            Style::default().fg(palette.muted),
        ));
    }

    let mut time = vec![Span::raw(" ".repeat(prefix_width))];
    push_labeled_progress(
        &mut time,
        copy.time,
        metric_label_width,
        window.remaining_time_percent_at(SystemTime::now()),
        bar_width,
        [palette.time, palette.time],
        palette,
    );
    [quota, time]
}

fn fit_display_width(value: &str, max_width: usize) -> String {
    if value.width() <= max_width {
        return value.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }

    let content_width = max_width - 1;
    let mut fitted = String::with_capacity(value.len());
    let mut used_width = 0;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if used_width + character_width > content_width {
            break;
        }
        fitted.push(character);
        used_width += character_width;
    }
    fitted.push('…');
    fitted
}

fn render_line(frame: &mut Frame<'_>, area: Rect, spans: Vec<Span<'static>>, background: Color) {
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(background)),
        area,
    );
}

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y.saturating_add(y), area.width, 1)
}

fn fit_name(name: &str, width: usize) -> String {
    let mut value = name.chars().take(width).collect::<String>();
    let count = value.chars().count();
    if count < width {
        value.push_str(&" ".repeat(width - count));
    }
    value
}

fn pad_left_display(value: &str, width: usize) -> String {
    format!(
        "{}{}",
        " ".repeat(width.saturating_sub(value.width())),
        value
    )
}

fn progress_marker(percent: Option<u8>) -> &'static str {
    match percent {
        Some(50..=100) => FILLED_BAR_GLYPH,
        Some(_) => EMPTY_BAR_GLYPH,
        None => "·",
    }
}

fn compact_progress_value(percent: Option<u8>) -> String {
    percent.map_or_else(|| "--".to_owned(), |value| format!("{value}%"))
}

fn percent_color(window: &UsageWindow, palette: Palette) -> Color {
    if window.status == UsageStatus::Unavailable {
        return palette.muted;
    }
    match window.remaining_percent {
        50..=100 => palette.healthy,
        20..=49 => palette.warning,
        _ => palette.critical,
    }
}

fn period_label(period: Option<Duration>, language: Language) -> String {
    let Some(period) = period else {
        return language.copy().quota.to_owned();
    };
    let hours = period.as_secs() / 3600;
    match (language, hours >= 24 && hours % 24 == 0) {
        (Language::English, true) => format!("{}d", hours / 24),
        (Language::English, false) => format!("{hours}h"),
        (Language::Chinese, true) => format!("{}天", hours / 24),
        (Language::Chinese, false) => format!("{hours}时"),
    }
}

fn long_period_label(period: Option<Duration>, language: Language) -> String {
    let Some(period) = period else {
        return language.copy().quota.to_owned();
    };
    let hours = period.as_secs() / 3600;
    let (value, unit) = if hours >= 24 && hours % 24 == 0 {
        (hours / 24, "day")
    } else {
        (hours, "hour")
    };
    match language {
        Language::English => count_with_unit(value, unit),
        Language::Chinese if unit == "day" => format!("{value} 天"),
        Language::Chinese => format!("{value} 小时"),
    }
}

fn phase_text(phase: PlanPhase, language: Language) -> &'static str {
    let copy = language.copy();
    match phase {
        PlanPhase::Loading => copy.loading,
        PlanPhase::Refreshing => "↻",
        PlanPhase::Ready => "",
        PlanPhase::Stale => copy.stale,
        PlanPhase::Unavailable => copy.unavailable,
    }
}

fn freshness(fetched_at: SystemTime, language: Language) -> String {
    let age = SystemTime::now()
        .duration_since(fetched_at)
        .unwrap_or(Duration::ZERO);
    let seconds = age.as_secs();
    if seconds < 60 {
        return match language {
            Language::English => "Updated just now".to_owned(),
            Language::Chinese => "刚刚更新".to_owned(),
        };
    }
    let (value, unit) = if seconds < 3600 {
        (seconds / 60, "minute")
    } else if seconds < 86_400 {
        (seconds / 3600, "hour")
    } else {
        (seconds / 86_400, "day")
    };
    match language {
        Language::English => format!("Updated {} ago", count_with_unit(value, unit)),
        Language::Chinese if unit == "minute" => format!("{value} 分钟前更新"),
        Language::Chinese if unit == "hour" => format!("{value} 小时前更新"),
        Language::Chinese => format!("{value} 天前更新"),
    }
}

fn reset_text(resets_at: Option<SystemTime>, language: Language) -> String {
    let Some(resets_at) = resets_at else {
        return match language {
            Language::English => "Reset time unknown".to_owned(),
            Language::Chinese => "重置时间未知".to_owned(),
        };
    };
    let Ok(remaining) = resets_at.duration_since(SystemTime::now()) else {
        return match language {
            Language::English => "Reset".to_owned(),
            Language::Chinese => "已重置".to_owned(),
        };
    };
    let seconds = remaining.as_secs();
    if seconds < 60 {
        return match language {
            Language::English => "Resets within 1 minute".to_owned(),
            Language::Chinese => "1 分钟内重置".to_owned(),
        };
    }
    let (value, unit) = if seconds < 3600 {
        (seconds / 60, "minute")
    } else if seconds < 86_400 {
        (seconds / 3600, "hour")
    } else {
        (seconds / 86_400, "day")
    };
    match language {
        Language::English => format!("Resets in {}", count_with_unit(value, unit)),
        Language::Chinese if unit == "minute" => format!("{value} 分钟后重置"),
        Language::Chinese if unit == "hour" => format!("{value} 小时后重置"),
        Language::Chinese => format!("{value} 天后重置"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::PlanEvent,
        domain::{CodingPlan, PlanIdentity},
        theme::Theme,
    };
    use ratatui::{backend::TestBackend, Terminal};

    use insta::assert_snapshot;

    fn test_palette() -> Palette {
        Theme::Rainbow.palette()
    }

    fn plan(id: &str, provider_id: &str, display_name: &str, values: &[u8]) -> CodingPlan {
        CodingPlan {
            id: id.to_owned(),
            provider_id: provider_id.to_owned(),
            display_name: display_name.to_owned(),
            fetched_at: SystemTime::now(),
            windows: values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let is_codex = id == "codex";
                    let label = match (is_codex, index) {
                        (true, 0) => "Codex · 7 days",
                        (true, 1) => "GPT-5.3-Codex-Spark · 5 hours",
                        (true, _) => "GPT-5.3-Codex-Spark · 7 days",
                        (false, 0) => "Claude · 5 hours",
                        (false, _) => "Claude · 7 days",
                    };
                    let period = match (is_codex, index) {
                        (true, 0) | (true, 2..) | (false, 1..) => {
                            Duration::from_secs(7 * 24 * 60 * 60)
                        }
                        _ => Duration::from_secs(5 * 60 * 60),
                    };
                    UsageWindow {
                        id: format!("{id}:{index}"),
                        label: label.to_owned(),
                        period: Some(period),
                        remaining_percent: *value,
                        resets_at: Some(SystemTime::now() + Duration::from_secs(7200)),
                        status: UsageStatus::Available,
                    }
                })
                .collect(),
        }
    }

    fn app_with_codex_windows(values: &[u8]) -> App {
        let codex = PlanIdentity::new("codex", "openai", "Codex");
        let claude = PlanIdentity::new("claude", "anthropic", "Claude");
        let mut app = App::new([codex.clone(), claude.clone()]);
        app.set_language(Language::English);
        app.apply_event(PlanEvent::Fetched {
            identity: codex,
            result: Ok(plan("codex", "openai", "Codex", values)),
        });
        app.apply_event(PlanEvent::Fetched {
            identity: claude,
            result: Ok(plan("claude", "anthropic", "Claude", &[63, 82])),
        });
        app
    }

    fn populated_app() -> App {
        app_with_codex_windows(&[71, 52])
    }

    fn draw(app: &App, width: u16, height: u16) -> TestBackend {
        draw_with_history(app, &UsageHistory::empty(), width, height)
    }

    fn draw_with_history(
        app: &App,
        history: &UsageHistory,
        width: u16,
        height: u16,
    ) -> TestBackend {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app, history)).unwrap();
        terminal.backend().clone()
    }

    fn text(backend: &TestBackend, y: u16) -> String {
        (0..backend.buffer().area.width)
            .map(|x| backend.buffer().cell((x, y)).unwrap().symbol())
            .collect::<String>()
    }
    fn snapshot_text(backend: &TestBackend) -> String {
        (0..backend.buffer().area.height)
            .map(|y| text(backend, y).trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn usage_history(values: &[&[u8]]) -> UsageHistory {
        let mut history = UsageHistory::empty();
        for (index, values) in values.iter().enumerate() {
            let mut snapshot = plan("codex", "openai", "Codex", values);
            snapshot.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(index as u64 + 1);
            history.record(&snapshot).unwrap();
        }
        history
    }

    fn contains_braille(value: &str) -> bool {
        value
            .chars()
            .any(|character| ('\u{2800}'..='\u{28ff}').contains(&character))
    }

    fn bar_color(backend: &TestBackend, y: u16) -> Color {
        (0..backend.buffer().area.width)
            .find_map(|x| {
                let cell = backend.buffer().cell((x, y)).unwrap();
                (cell.symbol() == FILLED_BAR_GLYPH).then_some(cell.fg)
            })
            .expect("row should contain a filled progress track")
    }

    #[test]
    fn two_plans_fit_in_four_comparison_rows() {
        let backend = draw(&populated_app(), 100, 5);
        assert!(text(&backend, 0).contains("Codex") && text(&backend, 0).contains("Q "));
        assert!(text(&backend, 1).contains("T ") && !text(&backend, 1).contains("Q "));
        assert!(text(&backend, 2).contains("Claude") && text(&backend, 2).contains("Q "));
        assert!(text(&backend, 3).contains("T ") && !text(&backend, 3).contains("Q "));
    }

    #[test]
    fn narrow_layout_keeps_vertical_comparison_for_every_plan() {
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 24, 5);
        let codex_quota = text(&backend, 0).replace(' ', "");
        let codex_time = text(&backend, 1).replace(' ', "");
        let claude_quota = text(&backend, 2).replace(' ', "");
        let claude_time = text(&backend, 3).replace(' ', "");
        assert!(
            codex_quota.contains("CodexQ")
                && codex_quota.contains("71%")
                && codex_quota.contains("52%")
                && codex_quota.contains("33%"),
            "{codex_quota:?}"
        );
        assert!(
            codex_time.contains('T') && codex_time.contains("1%") && codex_time.contains("40%"),
            "{codex_time:?}"
        );
        assert!(
            claude_quota.contains("ClaudeQ")
                && claude_quota.contains("63%")
                && claude_quota.contains("82%"),
            "{claude_quota:?}"
        );
        assert!(
            claude_time.contains('T') && claude_time.contains("40%") && claude_time.contains("1%"),
            "{claude_time:?}"
        );
    }

    #[test]
    fn three_windows_stack_quota_directly_above_time() {
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 60, 9);
        let pairs = [
            (0, 1, "Codex", "71%", "1%"),
            (2, 3, "Spark", "52%", "40%"),
            (4, 5, "Spark", "33%", "1%"),
        ];
        for (quota_row, time_row, model, quota_value, time_value) in pairs {
            let quota = text(&backend, quota_row);
            let time = text(&backend, time_row);
            assert!(
                quota.contains(model) && quota.contains("Q ") && quota.contains(quota_value),
                "{quota:?}"
            );
            assert!(!quota.contains("T "), "{quota:?}");
            assert!(time.contains("T ") && time.contains(time_value), "{time:?}");
            assert!(!time.contains("Q "), "{time:?}");
        }
        assert_eq!(
            bar_color(&backend, 0),
            provider_accent("openai", test_palette())
        );
        assert_eq!(bar_color(&backend, 2), test_palette().spark_short);
        assert_eq!(bar_color(&backend, 4), test_palette().spark_long);
        assert!(text(&backend, 6).contains("Claude"));
        assert!(text(&backend, 8).replace(' ', "").contains("EnterDetails"));
    }

    #[test]
    fn short_viewport_uses_two_compact_metric_rows_per_plan() {
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 60, 5);
        let codex_quota = text(&backend, 0);
        let codex_time = text(&backend, 1);
        let claude_quota = text(&backend, 2);
        let claude_time = text(&backend, 3);
        assert!(
            codex_quota.contains("Q") && codex_quota.contains("71%") && codex_quota.contains("33%")
        );
        assert!(codex_time.contains("T") && codex_time.contains("40%"));
        assert!(claude_quota.contains("Claude") && claude_quota.contains("Q"));
        assert!(claude_time.contains("T") && !claude_time.contains("Q"));
    }

    #[test]
    fn selection_scrolls_past_a_stacked_plan_as_one_card() {
        let mut app = app_with_codex_windows(&[71, 52, 33]);
        app.select_next();

        let backend = draw(&app, 60, 4);
        assert!(text(&backend, 0).contains("Claude"));
        assert!(text(&backend, 1).contains("T "));
        assert_eq!(backend.buffer().cell((0, 0)).unwrap().symbol(), "›");
    }
    #[test]
    fn selected_plan_stays_visible_when_the_list_scrolls() {
        let mut app = App::new([
            PlanIdentity::new("first", "openai", "First"),
            PlanIdentity::new("second", "anthropic", "Second"),
            PlanIdentity::new("third", "google", "Third"),
        ]);
        app.select_next();
        app.select_next();

        let backend = draw(&app, 40, 2);
        assert!(text(&backend, 1).contains("Third"));
        assert_eq!(backend.buffer().cell((0, 1)).unwrap().symbol(), "›");
    }

    #[test]
    fn selection_has_a_marker_and_surface_background() {
        let backend = draw(&populated_app(), 60, 5);
        assert_eq!(backend.buffer().cell((0, 0)).unwrap().symbol(), "›");
        assert_eq!(
            backend.buffer().cell((0, 0)).unwrap().bg,
            test_palette().surface
        );
        assert_eq!(
            backend.buffer().cell((0, 1)).unwrap().bg,
            test_palette().surface
        );
        assert_eq!(
            backend.buffer().cell((0, 2)).unwrap().bg,
            test_palette().background
        );
    }

    #[test]
    fn provider_names_use_provider_accents() {
        let backend = draw(&populated_app(), 60, 5);
        assert_eq!(
            backend.buffer().cell((2, 0)).unwrap().fg,
            provider_accent("openai", test_palette())
        );
        assert_eq!(
            backend.buffer().cell((2, 2)).unwrap().fg,
            provider_accent("anthropic", test_palette())
        );
    }

    #[test]
    fn detail_view_expands_windows_and_reset_information() {
        let mut app = populated_app();
        app.toggle_detail();
        let backend = draw(&app, 100, 6);
        let rendered = (0..6).map(|y| text(&backend, y)).collect::<String>();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("Codex·7days"), "{rendered:?}");
        assert!(compact.contains("GPT-5.3-Codex-Spark·5hours"));
        assert!(compact.contains("Quota") && compact.contains("Time"));
        assert_eq!(
            bar_color(&backend, 1),
            provider_accent("openai", test_palette())
        );
        assert_eq!(bar_color(&backend, 3), test_palette().spark_short);
        assert!(compact.contains("Resetsin"));
        let footer = text(&backend, 5);
        assert!(footer.replace(' ', "").contains("EscBack"), "{footer:?}");
    }

    #[test]
    fn list_footer_stays_on_last_row() {
        let backend = draw(&populated_app(), 80, 8);
        let footer = text(&backend, 7);
        assert!(
            footer.replace(' ', "").contains("EnterDetails"),
            "{footer:?}"
        );
        assert!(!text(&backend, 2).contains("Enter Details"));
    }

    #[test]
    fn rainbow_footer_label_uses_seven_distinct_colors() {
        let spans = footer_spans(
            "",
            Theme::Rainbow,
            Language::English,
            Language::English.copy(),
            Theme::Rainbow.palette(),
        );
        let colors = spans[1..8]
            .iter()
            .map(|span| span.style.fg)
            .collect::<Vec<_>>();
        assert_eq!(
            colors,
            vec![
                Some(Color::Rgb(255, 91, 146)),
                Some(Color::Rgb(255, 143, 96)),
                Some(Color::Rgb(255, 202, 87)),
                Some(Color::Rgb(80, 235, 190)),
                Some(Color::Rgb(64, 214, 255)),
                Some(Color::Rgb(91, 157, 255)),
                Some(Color::Rgb(181, 117, 255)),
            ]
        );
    }

    #[test]
    fn theme_cycle_changes_surface_accents_and_footer_label() {
        let mut app = populated_app();
        let rainbow = draw(&app, 100, 4);
        assert!(text(&rainbow, 3).contains("Rainbow"));
        assert_eq!(
            rainbow.buffer().cell((0, 0)).unwrap().bg,
            Theme::Rainbow.palette().surface
        );
        let rainbow_bar = bar_color(&rainbow, 0);

        app.cycle_theme();
        let midnight = draw(&app, 100, 4);
        assert!(text(&midnight, 3).contains("Midnight"));
        assert_eq!(
            midnight.buffer().cell((0, 0)).unwrap().bg,
            Theme::Midnight.palette().surface
        );
        assert_ne!(rainbow_bar, bar_color(&midnight, 0));

        app.cycle_theme();
        let mono = draw(&app, 100, 4);
        assert!(text(&mono, 3).contains("Mono"));
        assert_eq!(
            mono.buffer().cell((0, 0)).unwrap().bg,
            Theme::Mono.palette().surface
        );
    }

    #[test]
    fn language_cycle_rewrites_list_detail_and_footer_copy() {
        let mut app = populated_app();
        let english = draw(&app, 100, 8);
        assert!(text(&english, 7).contains("Select"));
        let english_list = (0..7).map(|y| text(&english, y)).collect::<String>();
        assert!(english_list.contains("Q ") && english_list.contains("T "));
        assert!(text(&english, 7).contains("EN"));

        app.cycle_language();
        let chinese = draw(&app, 100, 8);
        let chinese_footer = text(&chinese, 7).replace(' ', "");
        assert!(chinese_footer.contains("选择"));
        assert!(chinese_footer.contains("中文"));
        let chinese_list = (0..7)
            .map(|y| text(&chinese, y))
            .collect::<String>()
            .replace(' ', "");
        assert!(chinese_list.contains("7天"), "{chinese_list:?}");
        assert!(chinese_list.contains("5时"), "{chinese_list:?}");
        assert!(chinese_list.contains("额") && chinese_list.contains("时"));

        app.toggle_detail();
        let detail = draw(&app, 100, 6);
        let rendered = (0..6).map(|y| text(&detail, y)).collect::<String>();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("刚刚更新"), "{rendered:?}");
        assert!(compact.contains("小时后重置"), "{rendered:?}");
        assert!(compact.contains("配额") && compact.contains("时间"));
    }

    #[test]
    fn comparison_labels_stay_factual_in_both_languages() {
        for language in [Language::English, Language::Chinese] {
            let mut app = populated_app();
            app.set_language(language);
            let list = draw(&app, 100, 8);
            app.toggle_detail();
            let detail = draw(&app, 100, 6);
            let rendered = (0..8)
                .map(|y| text(&list, y))
                .chain((0..6).map(|y| text(&detail, y)))
                .collect::<String>();

            for forbidden in [
                "recommend",
                "enough",
                "excess",
                "save usage",
                "建议",
                "够用",
                "过剩",
                "省点",
            ] {
                assert!(!rendered.to_lowercase().contains(forbidden), "{rendered:?}");
            }
        }
    }

    #[test]
    fn unavailable_plan_does_not_hide_healthy_plan() {
        let first = PlanIdentity::new("codex", "openai", "Codex");
        let second = PlanIdentity::new("claude", "anthropic", "Claude");
        let mut app = App::new([first.clone(), second.clone()]);
        app.set_language(Language::English);
        app.apply_event(PlanEvent::Fetched {
            identity: first,
            result: Ok(plan("codex", "openai", "Codex", &[71])),
        });
        app.apply_event(PlanEvent::Fetched {
            identity: second,
            result: Err(AdapterError::new(
                AdapterErrorKind::TimedOut,
                "Claude statusline",
            )),
        });
        let backend = draw(&app, 60, 4);
        assert!(text(&backend, 0).contains("71%"));
        let unavailable_row = text(&backend, 2);
        assert!(
            unavailable_row.replace(' ', "").contains("Unavailable"),
            "{unavailable_row:?}"
        );
    }

    #[test]
    fn rendered_output_contains_no_identity_or_billing_fields() {
        let backend = draw(&populated_app(), 100, 5);
        let rendered = (0..5).map(|y| text(&backend, y)).collect::<String>();
        for forbidden in ["email", "account", "organization", "billing", "token"] {
            assert!(!rendered.to_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn detail_view_shows_real_history_braille_when_space_allows() {
        let mut history = UsageHistory::empty();
        let mut earlier = plan("codex", "openai", "Codex", &[71, 52]);
        earlier.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(1);
        history.record(&earlier).unwrap();
        let mut middle = plan("codex", "openai", "Codex", &[63, 50]);
        middle.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(2);
        history.record(&middle).unwrap();
        let mut later = plan("codex", "openai", "Codex", &[55, 48]);
        later.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(3);
        history.record(&later).unwrap();

        let mut app = app_with_codex_windows(&[55, 48]);
        app.toggle_detail();
        let backend = draw_with_history(&app, &history, 80, 8);
        let first_trend = text(&backend, 3);
        let second_trend = text(&backend, 6);
        let first_compact = first_trend.replace(' ', "");
        let second_compact = second_trend.replace(' ', "");
        assert!(contains_braille(&first_trend), "{first_trend:?}");
        assert!(contains_braille(&second_trend), "{second_trend:?}");
        assert!(first_compact.contains("Last2seconds"), "{first_trend:?}");
        assert!(first_compact.contains("71%→55%·−16%"), "{first_trend:?}");
        assert!(second_compact.contains("52%→48%·−4%"), "{second_trend:?}");
        assert!(!first_compact.contains("old→new"));
        assert!(text(&backend, 7).replace(' ', "").contains("EscBack"));
    }

    #[test]
    fn flat_history_uses_an_honest_text_summary() {
        let mut history = UsageHistory::empty();
        let mut earlier = plan("codex", "openai", "Codex", &[71, 52]);
        earlier.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(1);
        history.record(&earlier).unwrap();
        let mut later = plan("codex", "openai", "Codex", &[71, 52]);
        later.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(2);
        history.record(&later).unwrap();

        let mut app = populated_app();
        app.toggle_detail();
        let backend = draw_with_history(&app, &history, 80, 8);
        let trend = text(&backend, 3);
        let compact = trend.replace(' ', "");
        assert!(
            compact.contains("Last1second71%·flat·2samples"),
            "{trend:?}"
        );
        assert!(!contains_braille(&trend), "{trend:?}");
    }

    #[test]
    fn detail_view_labels_unseeded_history_without_faking_a_trend() {
        let mut app = populated_app();
        app.toggle_detail();
        let backend = draw(&app, 80, 8);
        let trend = text(&backend, 3);
        let compact = trend.replace(' ', "");
        assert!(compact.contains("Collectinghistory"), "{trend:?}");
        assert!(!contains_braille(&trend), "{trend:?}");
    }

    #[test]
    fn quota_increase_starts_a_new_visual_history_cycle() {
        let samples = [
            HistorySample {
                at_millis: 1,
                remaining_percent: 40,
            },
            HistorySample {
                at_millis: 2,
                remaining_percent: 35,
            },
            HistorySample {
                at_millis: 3,
                remaining_percent: 100,
            },
            HistorySample {
                at_millis: 4,
                remaining_percent: 95,
            },
        ];

        let current = current_cycle(&samples);
        assert_eq!(current.len(), 2);
        assert_eq!(current[0].remaining_percent, 100);
        assert_eq!(current[1].remaining_percent, 95);
    }

    #[test]
    fn three_window_detail_uses_trends_only_when_all_fit() {
        let mut history = UsageHistory::empty();
        let mut earlier = plan("codex", "openai", "Codex", &[71, 52, 33]);
        earlier.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(1);
        history.record(&earlier).unwrap();
        let mut middle = plan("codex", "openai", "Codex", &[70, 50, 32]);
        middle.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(2);
        history.record(&middle).unwrap();
        let mut later = plan("codex", "openai", "Codex", &[68, 49, 31]);
        later.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(3);
        history.record(&later).unwrap();

        let mut app = app_with_codex_windows(&[68, 49, 31]);
        app.toggle_detail();

        let tall = draw_with_history(&app, &history, 80, 11);
        for row_index in [3, 6, 9] {
            let row = text(&tall, row_index);
            let compact = row.replace(' ', "");
            assert!(contains_braille(&row), "{row:?}");
            assert!(
                compact.contains("Last2seconds") && compact.contains('→'),
                "{row:?}"
            );
        }
        assert!(text(&tall, 10).replace(' ', "").contains("EscBack"));

        let short = draw_with_history(&app, &history, 80, 8);
        let rendered = (0..8).map(|y| text(&short, y)).collect::<String>();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("Codex·7days"), "{rendered:?}");
        assert_eq!(compact.matches("GPT-5.3-Codex-Spark").count(), 2);
        assert!(!contains_braille(&rendered), "{rendered:?}");
        assert!(text(&short, 7).replace(' ', "").contains("EscBack"));
    }

    #[test]
    fn detail_columns_align_quota_directly_above_time() {
        let mut app = app_with_codex_windows(&[71, 52, 33]);
        app.toggle_detail();
        let backend = draw(&app, 100, 8);

        for (quota_row, time_row) in [(1, 2), (3, 4), (5, 6)] {
            let quota = text(&backend, quota_row);
            let time = text(&backend, time_row);
            assert!(
                quota.contains("Quota") && quota.contains("Resets in 1 hour"),
                "{quota:?}"
            );
            assert!(time.contains("Time") && !time.contains("Quota"), "{time:?}");
            let bar_start = |row| {
                (0..backend.buffer().area.width)
                    .find(|x| {
                        matches!(
                            backend.buffer().cell((*x, row)).unwrap().symbol(),
                            FILLED_BAR_GLYPH | EMPTY_BAR_GLYPH
                        )
                    })
                    .expect("detail metric row should contain a progress bar")
            };
            assert_eq!(bar_start(quota_row), bar_start(time_row));
        }
    }

    #[test]
    fn relative_time_copy_uses_chinese_units() {
        assert!(freshness(
            SystemTime::now() - Duration::from_secs(2 * 3600),
            Language::Chinese,
        )
        .contains("小时前更新"));
        assert_eq!(
            reset_text(
                Some(SystemTime::now() + Duration::from_secs(5 * 3600 + 5)),
                Language::Chinese,
            ),
            "5 小时后重置"
        );
    }
    #[test]
    fn snapshot_normal_list_at_80_by_8() {
        let backend = draw(&populated_app(), 80, 8);
        assert_snapshot!("normal_list_80x8", snapshot_text(&backend));
    }

    #[test]
    fn snapshot_narrow_list_at_40_by_5() {
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 40, 5);
        assert_snapshot!("narrow_list_40x5", snapshot_text(&backend));
    }

    #[test]
    fn snapshot_three_window_detail_at_60_by_8() {
        let mut app = app_with_codex_windows(&[71, 52, 33]);
        app.toggle_detail();
        let backend = draw(&app, 60, 8);
        assert_snapshot!("three_window_detail_60x8", snapshot_text(&backend));
    }

    #[test]
    fn snapshot_braille_trend_at_80_by_11() {
        let history = usage_history(&[&[71, 52, 33], &[70, 50, 32], &[68, 49, 31]]);
        let mut app = app_with_codex_windows(&[68, 49, 31]);
        app.toggle_detail();
        let backend = draw_with_history(&app, &history, 80, 11);
        assert_snapshot!("braille_trend_80x11", snapshot_text(&backend));
    }

    #[test]
    fn snapshot_flat_history_at_80_by_11() {
        let history = usage_history(&[&[71, 52, 33], &[71, 52, 33], &[71, 52, 33]]);
        let mut app = app_with_codex_windows(&[71, 52, 33]);
        app.toggle_detail();
        let backend = draw_with_history(&app, &history, 80, 11);
        assert_snapshot!("flat_history_80x11", snapshot_text(&backend));
    }

    #[test]
    fn snapshot_chinese_detail_at_80_by_11() {
        let history = usage_history(&[&[71, 52, 33], &[70, 50, 32], &[68, 49, 31]]);
        let mut app = app_with_codex_windows(&[68, 49, 31]);
        app.set_language(Language::Chinese);
        app.toggle_detail();
        let backend = draw_with_history(&app, &history, 80, 11);
        assert_snapshot!("chinese_detail_80x11", snapshot_text(&backend));
    }

    #[test]
    fn snapshot_stale_and_unavailable_at_60_by_6() {
        let codex = PlanIdentity::new("codex", "openai", "Codex");
        let claude = PlanIdentity::new("claude", "anthropic", "Claude");
        let mut app = App::new([codex.clone(), claude.clone()]);
        app.set_language(Language::English);
        app.apply_event(PlanEvent::Fetched {
            identity: codex.clone(),
            result: Ok(plan("codex", "openai", "Codex", &[71, 52])),
        });
        app.apply_event(PlanEvent::Fetched {
            identity: codex,
            result: Err(AdapterError::new(AdapterErrorKind::TimedOut, "Codex CLI")),
        });
        app.apply_event(PlanEvent::Fetched {
            identity: claude,
            result: Err(AdapterError::new(
                AdapterErrorKind::SnapshotMissing,
                "Claude statusline",
            )),
        });

        let backend = draw(&app, 60, 6);
        assert_snapshot!("stale_and_unavailable_60x6", snapshot_text(&backend));
    }

    #[test]
    fn diagnostic_copy_covers_every_safe_error_kind() {
        let cases = [
            (AdapterErrorKind::CommandNotFound, "未找到来源命令", "PATH"),
            (AdapterErrorKind::NotAuthenticated, "来源尚未登录", "登录"),
            (AdapterErrorKind::TimedOut, "来源响应超时", "网络"),
            (
                AdapterErrorKind::ProtocolChanged,
                "来源数据格式已变化",
                "升级",
            ),
            (
                AdapterErrorKind::SnapshotMissing,
                "尚无用量快照",
                "statusLine",
            ),
            (AdapterErrorKind::SnapshotExpired, "缓存快照已过期", "刷新"),
        ];

        for (kind, title, action_fragment) in cases {
            let error = AdapterError::new(kind, "Claude statusline");
            assert_eq!(diagnostic_title(kind, Language::Chinese), title);
            assert!(diagnostic_action(error, Language::Chinese).contains(action_fragment));
        }
    }

    #[test]
    fn unavailable_detail_shows_safe_reason_and_action() {
        let claude = PlanIdentity::new("claude", "anthropic", "Claude");
        let mut app = App::new([claude.clone()]);
        app.set_language(Language::English);
        app.apply_event(PlanEvent::Fetched {
            identity: claude,
            result: Err(AdapterError::new(
                AdapterErrorKind::SnapshotMissing,
                "Claude statusline",
            )),
        });
        app.toggle_detail();

        let backend = draw(&app, 80, 6);
        let rendered = (0..6).map(|y| text(&backend, y)).collect::<String>();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("Nousagesnapshotyet"), "{rendered:?}");
        assert!(rendered.contains("limitdeck ingest claude"), "{rendered:?}");
        for forbidden in ["token", "stderr", "/Users/", "@"] {
            assert!(!rendered.contains(forbidden), "{rendered:?}");
        }
    }
}
