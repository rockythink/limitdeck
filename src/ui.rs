use std::time::{Duration, SystemTime};

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::{
    app::{App, PlanPhase, PlanState},
    domain::{UsageStatus, UsageWindow},
    history::{HistorySample, UsageHistory},
    theme::{palette, provider_accent, Palette},
};

const FILLED_BAR_GLYPH: &str = "━";
const EMPTY_BAR_GLYPH: &str = "─";
const TREND_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const SPARK_SHORT_ACCENT: Color = Color::Rgb(42, 183, 184);
const SPARK_LONG_ACCENT: Color = Color::Rgb(64, 145, 214);

pub fn render(frame: &mut Frame<'_>, app: &App, history: &UsageHistory) {
    let area = frame.area();
    let palette = palette();
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
        render_detail(frame, content_area, app, history, palette);
    } else {
        render_plan_list(frame, content_area, app, palette);
    }

    if let Some(footer_area) = footer_area {
        let text = if app.is_detail_open() {
            "  Esc 返回 · r 刷新 · q 退出"
        } else {
            "  ↑↓/jk 选择 · Enter 详情 · r 刷新 · q 退出"
        };
        render_line(
            frame,
            footer_area,
            vec![Span::styled(text, Style::default().fg(palette.muted))],
            palette.background,
        );
    }
}

fn render_plan_list(frame: &mut Frame<'_>, area: Rect, app: &App, palette: Palette) {
    if area.width < 12 || area.height == 0 {
        return;
    }
    if app.plans().is_empty() {
        render_line(
            frame,
            row(area, 0),
            vec![Span::styled(
                "未发现 Coding Plan",
                Style::default().fg(palette.muted),
            )],
            palette.background,
        );
        if area.height > 1 {
            render_line(
                frame,
                row(area, 1),
                vec![Span::styled(
                    "先运行 Claude 状态行接入或安装 OMP",
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
        let rows = plan_rows(plan, area.width, viewport_height, selected, palette);
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
    if uses_stacked_rows(state, width, viewport_height) {
        state.plan.as_ref().map_or(1, |plan| plan.windows.len())
    } else {
        1
    }
}

fn uses_stacked_rows(state: &PlanState, width: u16, viewport_height: usize) -> bool {
    let Some(plan) = &state.plan else {
        return false;
    };
    width >= 40 && plan.windows.len() > 2 && plan.windows.len() <= viewport_height
}

fn plan_rows(
    state: &PlanState,
    width: u16,
    viewport_height: usize,
    selected: bool,
    palette: Palette,
) -> Vec<Vec<Span<'static>>> {
    if uses_stacked_rows(state, width, viewport_height) {
        return stacked_plan_rows(state, width, selected, palette);
    }
    vec![plan_row(state, width, selected, palette)]
}

fn stacked_plan_rows(
    state: &PlanState,
    width: u16,
    selected: bool,
    palette: Palette,
) -> Vec<Vec<Span<'static>>> {
    let accent = provider_accent(&state.identity.provider_id, palette);
    let name_width = 9usize;
    let model_width = 8usize;
    let prefix_width = 2 + name_width;
    let bar_width = usize::from(width)
        .saturating_sub(prefix_width + model_width + 11)
        .clamp(6, 24);
    let plan = state.plan.as_ref().expect("stacked rows require a plan");

    plan.windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let mut spans = if index == 0 {
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
            let row_accent = window_accent(window, accent);
            spans.push(Span::styled(
                fit_name(&compact_window_model(window), model_width),
                Style::default().fg(row_accent),
            ));
            let period = period_label(window.period);
            spans.push(Span::styled(
                if window.period.is_some() {
                    format!(" {period:>4} ")
                } else {
                    " 配额 ".to_owned()
                },
                Style::default().fg(palette.muted),
            ));
            push_bar(&mut spans, window, bar_width, row_accent, palette);

            let phase = phase_text(state.phase);
            if index == 0 && !phase.is_empty() && width >= 64 {
                spans.push(Span::styled(
                    format!(" {phase}"),
                    Style::default().fg(palette.muted),
                ));
            }
            spans
        })
        .collect()
}

fn plan_row(state: &PlanState, width: u16, selected: bool, palette: Palette) -> Vec<Span<'static>> {
    let accent = provider_accent(&state.identity.provider_id, palette);
    let mut spans = vec![Span::styled(
        if selected { "› " } else { "  " },
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    )];
    let name_width = if width < 32 { 7 } else { 9 };
    spans.push(Span::styled(
        fit_name(&state.identity.display_name, name_width),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    ));

    let Some(plan) = &state.plan else {
        spans.push(Span::styled(
            phase_text(state.phase),
            Style::default().fg(palette.muted),
        ));
        return spans;
    };

    if plan.windows.len() > 2 || width < 32 || !room_for_bars(width, name_width, plan.windows.len())
    {
        for (index, window) in plan.windows.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled("/", Style::default().fg(palette.muted)));
            }
            spans.push(Span::styled(
                compact_percent(window),
                Style::default().fg(percent_color(window, palette)),
            ));
        }
    } else {
        let bar_width = bar_width(width, name_width, plan.windows.len());
        for window in &plan.windows {
            spans.push(Span::styled(
                format!(" {} ", period_label(window.period)),
                Style::default().fg(palette.muted),
            ));
            push_bar(
                &mut spans,
                window,
                bar_width,
                window_accent(window, accent),
                palette,
            );
        }
    }

    let phase = phase_text(state.phase);
    if !phase.is_empty() && width >= 64 {
        spans.push(Span::styled(
            format!(" {phase}"),
            Style::default().fg(palette.muted),
        ));
    }
    spans
}

fn push_bar(
    spans: &mut Vec<Span<'static>>,
    window: &UsageWindow,
    bar_width: usize,
    accent: Color,
    palette: Palette,
) {
    let filled = usize::from(window.remaining_percent) * bar_width / 100;
    spans.push(Span::styled(
        FILLED_BAR_GLYPH.repeat(filled),
        Style::default().fg(accent),
    ));
    spans.push(Span::styled(
        EMPTY_BAR_GLYPH.repeat(bar_width - filled),
        Style::default().fg(palette.border),
    ));
    spans.push(Span::styled(
        format!(" {:>3}%", window.remaining_percent),
        Style::default().fg(percent_color(window, palette)),
    ));
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

fn window_accent(window: &UsageWindow, default: Color) -> Color {
    if !is_spark_window(window) {
        return default;
    }
    match window.period {
        Some(period) if period < Duration::from_secs(24 * 60 * 60) => SPARK_SHORT_ACCENT,
        _ => SPARK_LONG_ACCENT,
    }
}

fn render_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    history: &UsageHistory,
    palette: Palette,
) {
    if area.width < 12 || area.height == 0 {
        return;
    }
    let Some(state) = app.selected_plan() else {
        return;
    };
    let accent = provider_accent(&state.identity.provider_id, palette);
    let freshness = state
        .plan
        .as_ref()
        .map(|plan| freshness(plan.fetched_at))
        .unwrap_or_else(|| phase_text(state.phase).to_owned());
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
    if let Some(plan) = &state.plan {
        let show_trends = area.width >= 40
            && usize::from(area.height.saturating_sub(1)) >= plan.windows.len().saturating_mul(2);
        for window in &plan.windows {
            if next_y >= area.height {
                break;
            }
            let window_accent = window_accent(window, accent);
            let spans = detail_window(window, area.width, window_accent, palette);
            render_line(frame, row(area, next_y), spans, palette.background);
            next_y += 1;

            if show_trends && next_y < area.height {
                let samples = history.samples(&plan.provider_id, &plan.id, &window.id);
                render_line(
                    frame,
                    row(area, next_y),
                    trend_line(samples, area.width, window_accent, palette),
                    palette.background,
                );
                next_y += 1;
            }
        }
    } else if next_y < area.height {
        render_line(
            frame,
            row(area, next_y),
            vec![Span::styled(
                phase_text(state.phase),
                Style::default().fg(palette.muted),
            )],
            palette.background,
        );
    }
}

fn trend_line(
    samples: &[HistorySample],
    width: u16,
    accent: Color,
    palette: Palette,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        "    趋势  ".to_owned(),
        Style::default().fg(palette.muted),
    )];
    if samples.len() < 2 {
        spans.push(Span::styled(
            "收集中（至少 2 个样本）",
            Style::default().fg(palette.muted),
        ));
        return spans;
    }

    let graph_width = usize::from(width.saturating_sub(22)).clamp(8, 48);
    spans.push(Span::styled(
        sparkline(samples, graph_width),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        "  旧 → 新",
        Style::default().fg(palette.muted),
    ));
    spans
}

fn sparkline(samples: &[HistorySample], width: usize) -> String {
    let visible = samples.len().min(width);
    let start = samples.len() - visible;
    let mut graph = String::with_capacity(visible * 3);
    for sample in &samples[start..] {
        let level = usize::from(sample.remaining_percent) * (TREND_GLYPHS.len() - 1) / 100;
        graph.push(TREND_GLYPHS[level]);
    }
    graph
}

fn detail_window(
    window: &UsageWindow,
    width: u16,
    accent: Color,
    palette: Palette,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("  {}  ", window.label),
        Style::default().fg(palette.text),
    )];
    if width >= 40 {
        let bar_width = usize::from(width.saturating_sub(36)).clamp(4, 18);
        let filled = usize::from(window.remaining_percent) * bar_width / 100;
        spans.push(Span::styled(
            FILLED_BAR_GLYPH.repeat(filled),
            Style::default().fg(accent),
        ));
        spans.push(Span::styled(
            EMPTY_BAR_GLYPH.repeat(bar_width - filled),
            Style::default().fg(palette.border),
        ));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        compact_percent(window),
        Style::default().fg(percent_color(window, palette)),
    ));
    if width >= 48 {
        spans.push(Span::styled(
            format!("  {}", reset_text(window.resets_at)),
            Style::default().fg(palette.muted),
        ));
    }
    spans
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

fn room_for_bars(width: u16, name_width: usize, windows: usize) -> bool {
    windows > 0 && usize::from(width) >= 2 + name_width + windows * 10
}

fn bar_width(width: u16, name_width: usize, windows: usize) -> usize {
    if windows == 0 {
        return 0;
    }
    let fixed = 2 + name_width + windows * 8;
    usize::from(width)
        .saturating_sub(fixed)
        .checked_div(windows)
        .unwrap_or(0)
        .clamp(1, 8)
}

fn compact_percent(window: &UsageWindow) -> String {
    if window.status == UsageStatus::Available {
        format!("{}%", window.remaining_percent)
    } else {
        "--".to_owned()
    }
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

fn period_label(period: Option<Duration>) -> String {
    let Some(period) = period else {
        return "配额".to_owned();
    };
    let hours = period.as_secs() / 3600;
    if hours >= 24 && hours % 24 == 0 {
        format!("{}d", hours / 24)
    } else {
        format!("{hours}h")
    }
}

fn phase_text(phase: PlanPhase) -> &'static str {
    match phase {
        PlanPhase::Loading => "加载中",
        PlanPhase::Refreshing => "↻",
        PlanPhase::Ready => "",
        PlanPhase::Stale => "缓存",
        PlanPhase::Unavailable => "不可用",
    }
}

fn freshness(fetched_at: SystemTime) -> String {
    let age = SystemTime::now()
        .duration_since(fetched_at)
        .unwrap_or(Duration::ZERO);
    if age.as_secs() < 60 {
        "刚刚更新".to_owned()
    } else if age.as_secs() < 3600 {
        format!("{}m 前更新", age.as_secs() / 60)
    } else if age.as_secs() < 86_400 {
        format!("{}h 前更新", age.as_secs() / 3600)
    } else {
        format!("{}d 前更新", age.as_secs() / 86_400)
    }
}

fn reset_text(resets_at: Option<SystemTime>) -> String {
    let Some(resets_at) = resets_at else {
        return "重置时间未知".to_owned();
    };
    let Ok(remaining) = resets_at.duration_since(SystemTime::now()) else {
        return "已重置".to_owned();
    };
    if remaining.as_secs() < 60 {
        "1m 内重置".to_owned()
    } else if remaining.as_secs() < 3600 {
        format!("{}m 后重置", remaining.as_secs() / 60)
    } else if remaining.as_secs() < 86_400 {
        format!("{}h 后重置", remaining.as_secs() / 3600)
    } else {
        format!("{}d 后重置", remaining.as_secs() / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::PlanEvent,
        domain::{CodingPlan, PlanIdentity},
    };
    use ratatui::{backend::TestBackend, Terminal};

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
                        (true, 0) => "普通 Codex · 7 天",
                        (true, 1) => "GPT-5.3-Codex-Spark · 5 小时",
                        (true, _) => "GPT-5.3-Codex-Spark · 7 天",
                        (false, 0) => "5 小时窗口",
                        (false, _) => "7 天窗口",
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

    fn bar_color(backend: &TestBackend, y: u16) -> Color {
        (0..backend.buffer().area.width)
            .find_map(|x| {
                let cell = backend.buffer().cell((x, y)).unwrap();
                (cell.symbol() == FILLED_BAR_GLYPH).then_some(cell.fg)
            })
            .expect("row should contain a filled progress track")
    }

    #[test]
    fn two_plans_fit_in_two_rows() {
        let backend = draw(&populated_app(), 100, 2);
        assert!(text(&backend, 0).contains("Codex"));
        assert!(text(&backend, 1).contains("Claude"));
    }

    #[test]
    fn narrow_layout_keeps_every_plan_and_percentage() {
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 24, 2);
        assert!(text(&backend, 0).contains("Codex"));
        assert!(text(&backend, 0).contains("71%/52%/33%"));
        assert!(text(&backend, 1).contains("Claude"));
        assert!(text(&backend, 1).contains("63%/82%"));
    }

    #[test]
    fn three_windows_use_one_readable_bar_per_row() {
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 60, 5);
        let first = text(&backend, 0);
        let second = text(&backend, 1);
        let third = text(&backend, 2);

        assert!(
            first.matches("Codex").count() == 2 && first.contains("7d") && first.contains("71%"),
            "{first:?}"
        );
        assert!(
            second.contains("Spark") && second.contains("5h") && second.contains("52%"),
            "{second:?}"
        );
        assert!(
            third.contains("Spark") && third.contains("7d") && third.contains("33%"),
            "{third:?}"
        );
        assert_eq!(bar_color(&backend, 0), provider_accent("openai", palette()));
        assert_eq!(bar_color(&backend, 1), SPARK_SHORT_ACCENT);
        assert_eq!(bar_color(&backend, 2), SPARK_LONG_ACCENT);
        assert_ne!(bar_color(&backend, 0), bar_color(&backend, 1));
        assert_ne!(bar_color(&backend, 1), bar_color(&backend, 2));
        for row in [first, second, third] {
            assert!(
                row.contains(FILLED_BAR_GLYPH) && row.contains(EMPTY_BAR_GLYPH),
                "{row:?}"
            );
            assert!(row.contains('━') && row.contains('─'), "{row:?}");
        }
        assert!(text(&backend, 3).contains("Claude"));
        let footer = text(&backend, 4);
        assert!(footer.replace(' ', "").contains("Enter详情"), "{footer:?}");
    }

    #[test]
    fn short_viewport_keeps_three_windows_compact_and_complete() {
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 60, 2);
        assert!(text(&backend, 0).contains("71%/52%/33%"));
        assert!(text(&backend, 1).contains("Claude"));
    }

    #[test]
    fn selection_scrolls_past_a_stacked_plan_as_one_card() {
        let mut app = app_with_codex_windows(&[71, 52, 33]);
        app.select_next();

        let backend = draw(&app, 60, 4);
        assert!(text(&backend, 0).contains("Claude"));
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
        let backend = draw(&populated_app(), 60, 2);
        assert_eq!(backend.buffer().cell((0, 0)).unwrap().symbol(), "›");
        assert_eq!(backend.buffer().cell((0, 0)).unwrap().bg, palette().surface);
        assert_eq!(
            backend.buffer().cell((0, 1)).unwrap().bg,
            palette().background
        );
    }

    #[test]
    fn provider_names_use_provider_accents() {
        let backend = draw(&populated_app(), 60, 2);
        assert_eq!(
            backend.buffer().cell((2, 0)).unwrap().fg,
            provider_accent("openai", palette())
        );
        assert_eq!(
            backend.buffer().cell((2, 1)).unwrap().fg,
            provider_accent("anthropic", palette())
        );
    }

    #[test]
    fn detail_view_expands_windows_and_reset_information() {
        let mut app = populated_app();
        app.toggle_detail();
        let backend = draw(&app, 80, 5);
        let rendered = (0..5).map(|y| text(&backend, y)).collect::<String>();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("普通Codex·7天"), "{rendered:?}");
        assert!(compact.contains("GPT-5.3-Codex-Spark·5小时"));
        assert_eq!(bar_color(&backend, 1), provider_accent("openai", palette()));
        assert_eq!(bar_color(&backend, 2), SPARK_SHORT_ACCENT);
        assert!(compact.contains("后重置"));
        let footer = text(&backend, 4);
        assert!(footer.replace(' ', "").contains("Esc返回"), "{footer:?}");
    }

    #[test]
    fn list_footer_stays_on_last_row() {
        let backend = draw(&populated_app(), 80, 8);
        let footer = text(&backend, 7);
        assert!(footer.replace(' ', "").contains("Enter详情"), "{footer:?}");
        assert!(!text(&backend, 2).contains("Enter 详情"));
    }

    #[test]
    fn unavailable_plan_does_not_hide_healthy_plan() {
        let first = PlanIdentity::new("codex", "openai", "Codex");
        let second = PlanIdentity::new("claude", "anthropic", "Claude");
        let mut app = App::new([first.clone(), second.clone()]);
        app.apply_event(PlanEvent::Fetched {
            identity: first,
            result: Ok(plan("codex", "openai", "Codex", &[71])),
        });
        app.apply_event(PlanEvent::Fetched {
            identity: second,
            result: Err(crate::adapter::AdapterError),
        });
        let backend = draw(&app, 60, 2);
        assert!(text(&backend, 0).contains("71%"));
        let unavailable_row = text(&backend, 1);
        assert!(
            unavailable_row.replace(' ', "").contains("不可用"),
            "{unavailable_row:?}"
        );
    }

    #[test]
    fn rendered_output_contains_no_identity_or_billing_fields() {
        let backend = draw(&populated_app(), 100, 3);
        let rendered = (0..3).map(|y| text(&backend, y)).collect::<String>();
        for forbidden in ["email", "account", "organization", "billing", "token"] {
            assert!(!rendered.to_lowercase().contains(forbidden));
        }
    }
    #[test]
    fn detail_view_shows_real_history_sparklines_when_space_allows() {
        let mut history = UsageHistory::empty();
        let mut earlier = plan("codex", "openai", "Codex", &[71, 52]);
        earlier.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(1);
        history.record(&earlier).unwrap();
        let mut later = plan("codex", "openai", "Codex", &[55, 48]);
        later.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(2);
        history.record(&later).unwrap();

        let mut app = populated_app();
        app.toggle_detail();
        let backend = draw_with_history(&app, &history, 80, 7);
        let first_trend = text(&backend, 2);
        let second_trend = text(&backend, 4);
        let first_compact = first_trend.replace(' ', "");
        let second_compact = second_trend.replace(' ', "");
        assert!(first_compact.contains("趋势"), "{first_trend:?}");
        assert!(second_compact.contains("趋势"), "{second_trend:?}");
        assert!(TREND_GLYPHS
            .iter()
            .any(|glyph| first_trend.contains(*glyph)));
        assert!(first_compact.contains("旧→新"));
        assert!(text(&backend, 6).replace(' ', "").contains("Esc返回"));
    }

    #[test]
    fn detail_view_labels_unseeded_history_without_faking_a_trend() {
        let mut app = populated_app();
        app.toggle_detail();
        let backend = draw(&app, 80, 7);
        let trend = text(&backend, 2);
        let compact = trend.replace(' ', "");
        assert!(compact.contains("收集中"), "{trend:?}");
        assert!(!compact.contains("旧→新"), "{trend:?}");
    }

    #[test]
    fn three_window_detail_uses_trends_only_when_all_fit() {
        let mut history = UsageHistory::empty();
        let mut earlier = plan("codex", "openai", "Codex", &[71, 52, 33]);
        earlier.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(1);
        history.record(&earlier).unwrap();
        let mut later = plan("codex", "openai", "Codex", &[68, 49, 31]);
        later.fetched_at = std::time::UNIX_EPOCH + Duration::from_secs(2);
        history.record(&later).unwrap();

        let mut app = app_with_codex_windows(&[68, 49, 31]);
        app.toggle_detail();

        let tall = draw_with_history(&app, &history, 80, 8);
        for row_index in [2, 4, 6] {
            let row = text(&tall, row_index).replace(' ', "");
            assert!(row.contains("趋势") && row.contains("旧→新"), "{row:?}");
        }
        assert!(text(&tall, 7).replace(' ', "").contains("Esc返回"));

        let short = draw_with_history(&app, &history, 80, 5);
        let rendered = (0..5).map(|y| text(&short, y)).collect::<String>();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("普通Codex"), "{rendered:?}");
        assert_eq!(compact.matches("GPT-5.3-Codex-Spark").count(), 2);
        assert!(!compact.contains("趋势"), "{rendered:?}");
        assert!(text(&short, 4).replace(' ', "").contains("Esc返回"));
    }
}
