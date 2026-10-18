use super::{
    config, Block, BorderType, Borders, Frame, Line, List, ListItem, ListState, Rect, Span, Style,
    Table, TableState, Text, UIStateGuard,
};
use crate::state::WindowRegion;
use unicode_bidi::BidiInfo;

/// Placeholder for content that is still loading, explaining waits caused by Spotify's rate limit.
pub fn loading_text() -> String {
    match crate::client::rate_limit_remaining() {
        Some(remaining) => format!(
            "Loading... (rate limited by Spotify, retrying in {}s)",
            remaining.as_secs().max(1)
        ),
        None => "Loading...".to_string(),
    }
}

/// Construct and render a block.
///
/// This function should only be used to render a window's borders and its title.
/// It returns the rectangle to render the inner widgets inside the block.
pub fn construct_and_render_block(
    title: &str,
    theme: &config::Theme,
    borders: Borders,
    frame: &mut Frame,
    rect: Rect,
) -> Rect {
    let mut title = title.to_string();

    let configs = config::get_config();

    let (borders, border_type) = match configs.app_config.border_type {
        config::BorderType::Hidden | config::BorderType::Plain => (borders, BorderType::Plain),
        config::BorderType::Rounded => (borders, BorderType::Rounded),
        config::BorderType::Double => (borders, BorderType::Double),
        config::BorderType::Thick => (borders, BorderType::Thick),
    };

    let mut block = Block::default()
        .borders(borders)
        .border_style(theme.border())
        .border_type(border_type);

    let inner_rect = block.inner(rect);

    // Handle `BorderType::Hidden` after determining the inner rectangle
    // `Hidden` border can be done by setting the borders to be `NONE`.
    // NOTE: we want to handle the border after the inner rectangle computation,
    // so that paddings between windows are properly determined.
    if configs.app_config.border_type == config::BorderType::Hidden {
        block = block.borders(Borders::NONE);
        // add padding to the title to ensure the inner text is aligned with the title
        title = format!(" {title}");
    }

    // Set `title` for the block
    block = block.title(Span::styled(title, theme.block_title()));

    frame.render_widget(block, rect);
    inner_rect
}

/// Construct a generic list widget
pub fn construct_list_widget<'a>(
    theme: &config::Theme,
    items: Vec<(String, bool)>,
    is_active: bool,
    selected_index: Option<usize>,
) -> (List<'a>, usize) {
    construct_thumbnail_list_widget(theme, items, is_active, selected_index, None)
}

/// Construct a list widget whose rows leave room on the left for a thumbnail of `thumbnail_size`
/// cells (see [`render_list_thumbnails`]). `None` renders a plain list.
pub fn construct_thumbnail_list_widget<'a>(
    theme: &config::Theme,
    items: Vec<(String, bool)>,
    is_active: bool,
    selected_index: Option<usize>,
    thumbnail_size: Option<(u16, u16)>,
) -> (List<'a>, usize) {
    let configs = config::get_config();
    let n_items = items.len();

    (
        List::new(
            items
                .into_iter()
                .enumerate()
                .map(|(i, (s, is_playing))| {
                    let text = if is_active && configs.app_config.enable_relative_line_number {
                        if let Some(selected_index) = selected_index {
                            let diff = (i as isize - selected_index as isize).abs();
                            let width = std::cmp::min(n_items.to_string().len(), 2);
                            format!("{diff:>width$}  {s}")
                        } else {
                            s
                        }
                    } else {
                        s
                    };
                    let text = match thumbnail_size {
                        Some((width, height)) => thumbnail_row_text(&text, width, height),
                        None => Text::from(text),
                    };
                    ListItem::new(text).style(if is_playing {
                        theme.current_playing()
                    } else {
                        Style::default()
                    })
                })
                .collect::<Vec<_>>(),
        )
        .highlight_style(theme.selection(is_active)),
        n_items,
    )
}

/// Lay out a row's text next to its thumbnail, moving the details after the first ` • ` onto
/// the row's second line.
fn thumbnail_row_text(text: &str, width: u16, height: u16) -> Text<'static> {
    let pad = " ".repeat(width as usize + 1);
    let mut lines = match text.split_once(" • ") {
        Some((name, details)) if height > 1 => vec![
            Line::from(format!("{pad}{name}")),
            Line::from(format!("{pad}{details}")),
        ],
        _ => vec![Line::from(format!("{pad}{text}"))],
    };
    lines.resize(height as usize, Line::default());
    Text::from(lines)
}

/// Render the thumbnails of a list previously rendered into `rect`, scrolled to `offset`.
/// `urls[i]` is the thumbnail of the list's `i`-th item.
#[cfg_attr(not(feature = "image"), allow(unused_variables))]
pub fn render_list_thumbnails(
    frame: &mut Frame,
    ui: &mut UIStateGuard,
    rect: Rect,
    offset: usize,
    urls: &[Option<String>],
) {
    #[cfg(feature = "image")]
    {
        let ui = &mut **ui;
        ui.thumbnails
            .render_list(&ui.picker, frame, rect, offset, urls);
    }
}

/// adjust the `selected` position of a `ListState` if that position is invalid
fn adjust_list_state(state: &mut ListState, len: usize) {
    if let Some(p) = state.selected() {
        if p >= len {
            state.select(if len > 0 { Some(len - 1) } else { Some(0) });
        }
    } else if len > 0 {
        state.select(Some(0));
    }
}

/// Render a list window, returning where it was drawn.
pub fn render_list_window(
    frame: &mut Frame,
    widget: List,
    rect: Rect,
    len: usize,
    state: &mut ListState,
) -> WindowRegion {
    adjust_list_state(state, len);
    frame.render_stateful_widget(widget, rect, state);
    WindowRegion::new(rect, 0, state.offset(), len)
}

/// adjust the `selected` position of a `TableState` if that position is invalid
fn adjust_table_state(state: &mut TableState, len: usize) {
    if let Some(p) = state.selected() {
        if p >= len {
            state.select(if len > 0 { Some(len - 1) } else { Some(0) });
        }
    } else if len > 0 {
        state.select(Some(0));
    }
}

/// Render a table window (with a one-row header), returning where it was drawn.
pub fn render_table_window(
    frame: &mut Frame,
    widget: Table,
    rect: Rect,
    len: usize,
    state: &mut TableState,
) -> WindowRegion {
    adjust_table_state(state, len);
    frame.render_stateful_widget(widget, rect, state);
    WindowRegion::new(rect, 1, state.offset(), len)
}

/// Convert a string to a bidirectional string.
/// Used to handle RTL text properly in the UI.
pub fn to_bidi_string(s: &str) -> String {
    let bidi_info = BidiInfo::new(s, None);

    let bidi_string = if bidi_info.has_rtl() && !bidi_info.paragraphs.is_empty() {
        bidi_info
            .reorder_line(&bidi_info.paragraphs[0], 0..s.len())
            .into_owned()
    } else {
        s.to_string()
    };

    bidi_string
}

/// formats genres depending on the number of genres and `genre_num`
///
/// Examples for `genre_num = 2`
/// - 1 genre: "genre1"
/// - 2 genres: "genre1, genre2"
/// - \>= 3 genres: "genre1, genre2, ..."
pub fn format_genres(genres: &[String], genre_num: u8) -> String {
    let mut genre_str = String::with_capacity(64);

    if genre_num > 0 {
        for i in 0..genres.len() {
            genre_str.push_str(&genres[i]);

            if i + 1 != genres.len() {
                genre_str.push_str(", ");

                if i + 1 >= genre_num as usize {
                    genre_str.push_str("...");
                    break;
                }
            }
        }
    }

    genre_str
}
