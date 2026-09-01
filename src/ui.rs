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
    theme::{palette, provider_accent, Palette},
};

const FILLED_BAR_GLYPH: &str = "━";
const EMPTY_BAR_GLYPH: &str = "─";

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let palette = palette();
    frame.render_widget(
        Block::default().style(Style::default().bg(palette.background)),
        area,
    );

    if app.is_detail_open() {
        render_detail(frame, area, app, palette);
    } else {
        render_plan_list(frame, area, app, palette);
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

    if screen_row < viewport_height {
        render_line(
            frame,
            row(area, screen_row as u16),
            vec![Span::styled(
                "  ↑↓/jk 选择 · Enter 详情 · r 刷新 · q 退出",
                Style::default().fg(palette.muted),
            )],
            palette.background,
        );
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
    let prefix_width = 2 + name_width;
    let bar_width = usize::from(width)
        .saturating_sub(prefix_width + 11)
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
            let period = period_label(window.period);
            spans.push(Span::styled(
                if window.period.is_some() {
                    format!(" {period:>4} ")
                } else {
                    " 配额 ".to_owned()
                },
                Style::default().fg(palette.muted),
            ));
            push_bar(&mut spans, window, bar_width, accent, palette);

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
            push_bar(&mut spans, window, bar_width, accent, palette);
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

fn render_detail(frame: &mut Frame<'_>, area: Rect, app: &App, palette: Palette) {
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
        for window in plan
            .windows
            .iter()
            .take(area.height.saturating_sub(1) as usize)
        {
            let spans = detail_window(window, area.width, accent, palette);
            render_line(frame, row(area, next_y), spans, palette.background);
            next_y += 1;
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
        next_y += 1;
    }

    if next_y < area.height {
        render_line(
            frame,
            row(area, next_y),
            vec![Span::styled(
                "  Esc 返回 · r 刷新 · q 退出",
                Style::default().fg(palette.muted),
            )],
            palette.background,
        );
    }
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
                .map(|(index, value)| UsageWindow {
                    id: format!("{id}:{index}"),
                    label: if index == 0 {
                        "5 小时窗口".to_owned()
                    } else {
                        "7 天窗口".to_owned()
                    },
                    period: Some(if index == 0 {
                        Duration::from_secs(5 * 60 * 60)
                    } else {
                        Duration::from_secs(7 * 24 * 60 * 60)
                    }),
                    remaining_percent: *value,
                    resets_at: Some(SystemTime::now() + Duration::from_secs(7200)),
                    status: UsageStatus::Available,
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
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().clone()
    }

    fn text(backend: &TestBackend, y: u16) -> String {
        (0..backend.buffer().area.width)
            .map(|x| backend.buffer().cell((x, y)).unwrap().symbol())
            .collect::<String>()
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
        let backend = draw(&app_with_codex_windows(&[71, 52, 33]), 60, 4);
        let first = text(&backend, 0);
        let second = text(&backend, 1);
        let third = text(&backend, 2);

        assert!(
            first.contains("Codex") && first.contains("71%"),
            "{first:?}"
        );
        assert!(second.contains("52%"), "{second:?}");
        assert!(third.contains("33%"), "{third:?}");
        assert!(!first.contains("52%"));
        assert!(!second.contains("33%"));
        for row in [first, second, third] {
            assert!(
                row.contains(FILLED_BAR_GLYPH) && row.contains(EMPTY_BAR_GLYPH),
                "{row:?}"
            );
            assert!(!row.contains('█') && !row.contains('░'), "{row:?}");
        }
        assert!(text(&backend, 3).contains("Claude"));
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

        let backend = draw(&app, 60, 3);
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
        assert!(compact.contains("5小时窗口"), "{rendered:?}");
        assert!(compact.contains("7天窗口"));
        assert!(compact.contains("后重置"));
        assert!(compact.contains("Esc返回"));
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
}
