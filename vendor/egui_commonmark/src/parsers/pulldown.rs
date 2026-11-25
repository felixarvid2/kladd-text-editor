use std::borrow::ToOwned;
use std::iter::Peekable;
use std::ops::Range;

use crate::{CommonMarkCache, CommonMarkOptions};

#[cfg(feature = "regex")]
use egui::RichText;
use egui::text::LayoutJob;
use egui::{self, Id, Pos2, TextStyle, Ui};

use crate::List;
use egui_commonmark_backend::elements::*;
use egui_commonmark_backend::misc::*;
use egui_commonmark_backend::pulldown::*;
#[cfg(feature = "regex")]
use egui_commonmark_backend::search::search_intervals;
use pulldown_cmark::{CowStr, HeadingLevel};

/// Newline logic is constructed by the following:
/// All elements try to insert a newline before them (if they are allowed)
/// and end their own line.
struct Newline {
    /// Whether a newline should not be inserted before a widget. This is only for
    /// the first widget.
    should_not_start_newline_forced: bool,
    /// Whether an element should insert a newline before it
    should_start_newline: bool,
    /// Whether an element should end it's own line using a newline
    /// This will have to be set to false in cases such as when blocks are within
    /// a list.
    should_end_newline: bool,
    /// only false when the widget is the last one.
    should_end_newline_forced: bool,
}

impl Default for Newline {
    fn default() -> Self {
        Self {
            should_not_start_newline_forced: true,
            should_start_newline: true,
            should_end_newline: true,
            should_end_newline_forced: true,
        }
    }
}

impl Newline {
    pub fn can_insert_end(&self) -> bool {
        self.should_end_newline && self.should_end_newline_forced
    }

    pub fn can_insert_start(&self) -> bool {
        self.should_start_newline && !self.should_not_start_newline_forced
    }

    pub fn try_insert_start(&self, ui: &mut Ui) {
        if self.can_insert_start() {
            newline(ui);
        }
    }

    pub fn try_insert_end(&self, ui: &mut Ui) {
        if self.can_insert_end() {
            newline(ui);
        }
    }
}

#[derive(Default)]
struct DefinitionList {
    is_first_item: bool,
    is_def_list_def: bool,
}

pub struct CommonMarkViewerInternal {
    curr_table: usize,
    text_style: Style,
    list: List,
    link: Option<Link>,
    image: Option<Image>,
    line: Newline,
    code_block: Option<CodeBlock>,

    /// Only populated if the html_fn option has been set
    html_block: String,
    is_list_item: bool,
    def_list: DefinitionList,
    is_table: bool,
    is_blockquote: bool,
    checkbox_events: Vec<CheckboxClickEvent>,
    deferred_scroll_to_heading: Option<String>,
    /// Set during a full render if any image rendered at zero height (texture
    /// still loading). When true, split points are discarded and the full render
    /// repeats next frame until all images have stable heights.
    any_image_loading: bool,
    /// Taken from [`ScrollCache::take_pending_scroll_to_active_match`] at
    /// the start of a render pass. When true, the renderer scrolls to (and
    /// centers) the active search match the moment it is found, then clears
    /// this flag so it only happens once per pass.
    want_scroll_to_active_match: bool,
    /// Screen-space y of the document content origin for this render pass,
    /// captured once at the top of `show()`. Subtracting this from any
    /// widget's screen y gives a scroll-independent virtual y.
    content_origin_y: f32,
    /// `(global_match_index, virtual_y)` pairs accumulated across all
    /// `event_text` calls during this render pass. Flushed into
    /// `SearchCache` [`CommonMarkCache`] at the end of `show()` (non-scrollable path only).
    search_match_ys_scratch: Vec<(usize, f32)>,
    /// Kladd: den förra händelsen var slutet på ett stycke (för att hålla ihop indragna stycken).
    last_end_para: bool,
}

pub(crate) struct CheckboxClickEvent {
    pub(crate) checked: bool,
    pub(crate) span: Range<usize>,
}

impl CommonMarkViewerInternal {
    pub fn new() -> Self {
        Self {
            curr_table: 0,
            text_style: Style::default(),
            list: List::default(),
            link: None,
            image: None,
            line: Newline::default(),
            is_list_item: false,
            def_list: Default::default(),
            code_block: None,
            html_block: String::new(),
            is_table: false,
            is_blockquote: false,
            checkbox_events: Vec::new(),
            deferred_scroll_to_heading: None,
            any_image_loading: false,
            want_scroll_to_active_match: false,
            content_origin_y: 0.0,
            search_match_ys_scratch: Vec::new(),
            last_end_para: false,
        }
    }
}

fn parser_options_extras(
    is_math_enabled: bool,
    is_scroll_to_heading_enabled: bool,
) -> pulldown_cmark::Options {
    let mut result = parser_options();
    if is_math_enabled {
        result |= pulldown_cmark::Options::ENABLE_MATH;
    }
    if is_scroll_to_heading_enabled {
        result |= pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES;
    }
    result
}

/// Tracks how many full-document renders have been performed for each
/// `show_scrollable` source, keyed by `split_points_id`. Only compiled in
/// test builds; used by the perf-regression tests below to assert that the
/// cheap viewport-only path is taken once split points have been populated.
/// Because each source uses its own key, tests that run in parallel do not
/// interfere with each other's counts.
#[cfg(test)]
static FULL_RENDER_COUNTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<Id, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

impl CommonMarkViewerInternal {
    /// Be aware that this acquires egui::Context internally.
    /// If split Id is provided then split points will be populated
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        text: &str,
        split_points_id: Option<Id>,
    ) -> (egui::InnerResponse<()>, Vec<CheckboxClickEvent>) {
        #[cfg(test)]
        if let Some(sid) = split_points_id {
            *FULL_RENDER_COUNTS.lock().unwrap().entry(sid).or_insert(0) += 1;
        }

        let max_width = options.max_width(ui);

        // Determine the effective id for split-point recording this frame.
        // `show_scrollable`'s full render passes its `source_id` as `split_points_id`
        // and always records.
        // `show_with_id` stores its `source_id` in `CommonMarkOptions` and rebuilds
        // only when the available width changes; stable frames reuse cached split points.
        let record_id: Option<Id> = split_points_id.or_else(|| {
            options.source_id.and_then(|id| {
                let vc = viewer_cache(cache, &id);
                let needs_rebuild =
                    vc.split_points.is_empty() || (vc.available_size.x - max_width).abs() > 0.5;
                if needs_rebuild {
                    vc.split_points.clear();
                    vc.heading_y_positions.clear();
                    vc.available_size.x = max_width;
                    Some(id)
                } else {
                    None // existing split points are still valid; skip re-recording
                }
            })
        });

        self.any_image_loading = false;

        // Take the pending scroll-to-active-match every frame, not only on
        // rebuild frames (when record_id is Some). On steady-state frames
        // record_id is None (split points are cached) but the user may have
        // just pressed Next/Prev, so we must still honour the request.
        #[cfg(feature = "regex")]
        {
            let scroll_source_id = split_points_id.or(options.source_id);
            self.want_scroll_to_active_match = scroll_source_id.is_some_and(|id| {
                viewer_cache(cache, &id)
                    .search_cache
                    .take_pending_scroll_to_active_match()
            });
            self.search_match_ys_scratch.clear();
        }

        let layout = egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true);

        let re = ui.allocate_ui_with_layout(egui::vec2(max_width, 0.0), layout, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let height = ui.text_style_height(&TextStyle::Body);
            ui.set_row_height(height);

            // Do a full render
            let content_origin_y = self.full_render(cache, options, text, record_id, max_width, ui);

            {
                let vc = viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL));
                // deferral to make it consistent no matter whether the target is before or after the link
                *vc.scroll_to_id_target_mut() = self.deferred_scroll_to_heading.take();
            }

            if let Some(source_id) = split_points_id {
                if self.any_image_loading {
                    // Images are still loading — split points are unreliable.
                    // Discard and leave page_size = None so the full render repeats
                    // next frame (the image loader triggers the repaint automatically).
                    let vc = viewer_cache(cache, &source_id);
                    vc.split_points.clear();
                    vc.heading_y_positions.clear();
                } else {
                    let final_y = ui.next_widget_position().y;
                    viewer_cache(cache, &source_id).page_size =
                        Some(egui::vec2(max_width, final_y - content_origin_y));
                }
            } else {
                // Non-scrollable show() path: flush the per-match virtual-y positions
                // and the current viewport top so that callers can sync the active
                // match after the user scrolls.
                #[cfg(feature = "regex")]
                {
                    let viewport_top_y = ui.clip_rect().min.y - content_origin_y;
                    let viewport_height = ui.clip_rect().height();
                    let vc = viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL));
                    vc.search_cache.update_show_viewport(
                        self.search_match_ys_scratch.drain(..),
                        viewport_top_y,
                        viewport_height,
                    );
                    // For show_with_id: keep the ViewerCache's viewport position in sync
                    // so that `viewport_start_byte_offset` returns the current scroll location.
                    vc.search_cache.last_viewport_top_y = viewport_top_y;
                }
            }
        });

        (re, std::mem::take(&mut self.checkbox_events))
    }

    /// Perform a full render of the document.
    fn full_render(
        &mut self,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions<'_>,
        text: &str,
        record_id: Option<Id>,
        max_width: f32,
        ui: &mut Ui,
    ) -> f32 {
        let mut events = pulldown_cmark::Parser::new_ext(
            text,
            parser_options_extras(options.math_fn.is_some(), options.enable_scroll_to_heading),
        )
        .into_offset_iter()
        .enumerate()
        .peekable();

        // Screen-space Y of the content origin. Subtracting this from any
        // cursor position gives virtual (content-relative) Y comparable to
        // `viewport.min/max.y` from `show_viewport`.
        let content_origin_y = ui.next_widget_position().y;
        self.content_origin_y = content_origin_y;

        // Cursor at the visual top of the current block, captured at
        // `Start(Block)` so that `vstart` reflects the block top, not its bottom.
        let mut block_start_position: Option<Pos2> = None;
        // Source byte offset paired with block_start_position, so split
        // points can also record each block's source span (used to
        // locate search matches without a fresh full render).
        let mut block_start_src: Option<usize> = None;

        let line_sid = options.source_id.unwrap_or(Id::NULL);
        viewer_cache(cache, &line_sid).line_points.clear();

        while let Some((index, (e, src_span))) = events.next() {
            let start_position = ui.next_widget_position();

            // Kladd: var varje block börjar, för dubbelklick och rullningssynk.
            // HTML-block, tabeller och linjer räknas också: ett dokument som börjar med en HTML-block
            // (`<p align="center">`) hade annars ingen position förrän första stycket, och dubbelklicket
            // där hittade ingen rad.
            if matches!(
                e,
                pulldown_cmark::Event::Rule
                    | pulldown_cmark::Event::Start(
                        pulldown_cmark::Tag::Paragraph
                            | pulldown_cmark::Tag::Heading { .. }
                            | pulldown_cmark::Tag::CodeBlock(_)
                            | pulldown_cmark::Tag::Item
                            | pulldown_cmark::Tag::HtmlBlock
                            | pulldown_cmark::Tag::Table(_)
                            | pulldown_cmark::Tag::DefinitionListTitle
                    )
            ) {
                viewer_cache(cache, &line_sid)
                    .line_points
                    .push((src_span.start, start_position.y - content_origin_y));
            }

            let is_safe_block_start = !self.list.is_inside_a_list()
                && matches!(
                    e,
                    pulldown_cmark::Event::Start(
                        pulldown_cmark::Tag::Paragraph
                            | pulldown_cmark::Tag::Heading { .. }
                            | pulldown_cmark::Tag::CodeBlock(_)
                    )
                );
            if is_safe_block_start {
                block_start_position = Some(start_position);
                block_start_src = Some(src_span.start);
            }

            // Record virtual Y for each named heading so the viewport path
            // can jump to headings that are outside the rendered slice.
            if let (
                Some(sid),
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::Heading { id: Some(id), .. }),
            ) = (record_id, &e)
            {
                viewer_cache(cache, &sid)
                    .heading_y_positions
                    .insert(id.to_string(), ui.cursor().min.y - content_origin_y);
            }

            // Only record split points at clean, top-level block boundaries
            // where the renderer has no pending state and can restart safely.
            let is_safe_block_end = !self.list.is_inside_a_list()
                && matches!(
                    e,
                    pulldown_cmark::Event::End(
                        pulldown_cmark::TagEnd::Paragraph
                            | pulldown_cmark::TagEnd::Heading { .. }
                            | pulldown_cmark::TagEnd::CodeBlock
                    )
                );

            if events.peek().is_none() {
                self.line.should_end_newline_forced = false;
            }

            let block_end_src = src_span.end;
            self.process_event(ui, &mut events, e, src_span, cache, options, max_width);

            if let Some(source_id) = record_id
                && is_safe_block_end
            {
                let vc = viewer_cache(cache, &source_id);
                let end_position = ui.next_widget_position();

                let split_point_exists = vc.split_points.iter().any(|sp| sp.event_index == index);

                if !split_point_exists {
                    // Use `block_start_position` (`Start` event) not `start_position`
                    // (cursor just before `End`) so that `vstart` is the block top.
                    let raw_vstart = block_start_position.take().unwrap_or(start_position);
                    let vstart = egui::pos2(raw_vstart.x, raw_vstart.y - content_origin_y);
                    let vend = egui::pos2(end_position.x, end_position.y - content_origin_y);
                    let src_span = block_start_src.take().unwrap_or(block_end_src)..block_end_src;
                    vc.split_points.push(SplitPoint {
                        event_index: index,
                        vstart,
                        vend,
                        src_span,
                    });
                }
            }

            if index == 0 {
                self.line.should_not_start_newline_forced = false;
            }
        }
        content_origin_y
    }

    fn create_scroll_area(
        scroll_id: Id,
        scroll_options: &crate::CommonMarkScrollOptions,
    ) -> egui::ScrollArea {
        let mut scroll_area = egui::ScrollArea::new([scroll_options.horizontal_scrolling, true])
            .id_salt(scroll_id)
            // Elements have different widths, so the scroll area cannot try to shrink to the
            // content, as that will mean that the scroll bar will move when loading elements
            // with different widths.
            .auto_shrink([false, true])
            .scroll_source(scroll_options.scroll_source)
            .wheel_scroll_multiplier(scroll_options.wheel_scroll_multiplier);

        if let Some(margin) = scroll_options.content_margin {
            scroll_area = scroll_area.content_margin(margin);
        }

        scroll_area
    }

    pub(crate) fn show_scrollable(
        &mut self,
        source_id: Id,
        ui: &mut egui::Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        scroll_options: &crate::CommonMarkScrollOptions,
        text: &str,
    ) {
        let available_size = ui.available_size();
        let scroll_id = source_id.with("_scroll_area");

        if !scroll_options.use_viewport_cache {
            // Full-document render every frame; egui clips what is off-screen.
            // Split points are maintained for `viewport_start_byte_offset` (search
            // anchoring), rebuilt only on width change — matching egui's own
            // galley-cache invalidation and the `show_with_id` path.
            let needs_rebuild = {
                let vc = viewer_cache(cache, &source_id);
                vc.page_size = None;
                vc.heading_y_positions.clear(); // not used in this path
                let rebuild = vc.split_points.is_empty()
                    || (vc.available_size.x - available_size.x).abs() > 0.5;
                if rebuild {
                    vc.split_points.clear();
                    vc.available_size.x = available_size.x;
                }
                rebuild
            };

            Self::create_scroll_area(scroll_id, scroll_options).show(ui, |ui| {
                // Capture viewport top before content is placed; `clip_rect`
                // already reflects the current scroll position.
                #[cfg(feature = "regex")]
                let viewport_top_y = ui.clip_rect().min.y - ui.next_widget_position().y;
                let vc = viewer_cache(cache, &source_id);
                apply_pending_scroll_delta(vc, ui);
                let sid = if needs_rebuild { Some(source_id) } else { None };

                self.show(ui, cache, options, text, sid);

                if needs_rebuild {
                    // `show()` sets page_size as a side-effect of receiving
                    // `Some(source_id)`; clear it so the next frame still
                    // takes this full-render path, not the viewport-slice one.
                    viewer_cache(cache, &source_id).page_size = None;
                }

                // Keep last_viewport_top_y in sync for viewport_start_byte_offset.
                #[cfg(feature = "regex")]
                {
                    viewer_cache(cache, &source_id)
                        .search_cache
                        .last_viewport_top_y = viewport_top_y;
                }
            });
            return;
        }

        // If the viewer cache is invalidated, force a full render.
        // Extract page_size in a short scope so `cache` is free for the else closure.
        let page_size = viewer_cache(cache, &source_id).page_size;
        let Some(page_size) = page_size else {
            Self::create_scroll_area(scroll_id, scroll_options).show(ui, |ui| {
                apply_pending_scroll_delta(viewer_cache(cache, &source_id), ui);
                self.show(ui, cache, options, text, Some(source_id));
            });

            viewer_cache(cache, &source_id).available_size = available_size;
            return;
        };

        // Try to fulfil a pending scroll-to-active-match directly from the
        // slice we're about to render: if the match is already visible (the
        // common case, e.g. stepping through nearby results), the per-widget
        // rendering code below finds it and scrolls precisely, with no extra
        // cost. If it isn't in the rendered slice, `pending_match_scroll_y`
        // (below) provides a cheap blind scroll toward its approximate
        // position using data already collected by the last full render —
        // this never requires re-rendering the whole document.
        #[cfg(feature = "regex")]
        {
            self.want_scroll_to_active_match = viewer_cache(cache, &source_id)
                .search_cache
                .take_pending_scroll_to_active_match();
        }

        let events = pulldown_cmark::Parser::new_ext(
            text,
            parser_options_extras(options.math_fn.is_some(), options.enable_scroll_to_heading),
        )
        .into_offset_iter()
        .collect::<Vec<_>>();

        let num_rows = events.len();

        // Resolve any pending TOC scroll via the cached heading positions so that
        // navigation works even when the target is outside the rendered slice.
        let pending_scroll_y: Option<f32> = {
            let vc = viewer_cache(cache, &source_id);
            let slug_owned = vc.scroll_to_id_target().map(ToOwned::to_owned);
            slug_owned.as_ref().and_then(|slug| {
                if let Some(&y) = vc.heading_y_positions.get(slug) {
                    vc.scroll_to_id_target_mut().take();
                    Some(y)
                } else {
                    None
                }
            })
        };
        let pending_delta = {
            let vc = viewer_cache(cache, &source_id);
            std::mem::replace(&mut vc.pending_scroll_delta, egui::Vec2::ZERO)
        };

        // Virtual Y of the active match used to decide whether a blind scroll is needed
        // before the slice renders.
        //
        // Prefer the precise per-match Y recorded during the *previous* frame's render
        // (`search_match_virtual_ys`). That value is exact for any match that was on
        // screen last frame — including matches deep inside tall code blocks, where
        // `virtual_y_for_byte_offset` only returns the block-top Y. We avoid using
        // the block-top Y because it triggers a spurious blind scroll to the top of
        // the block followed by a second scroll to the match, producing a distracting
        // animation of scrolling to the previous page then back.
        //
        // Fall back to the block-level split-point approximation only when
        // the match was not rendered last frame (Y stored as NEG_INFINITY).
        // All viewer-cache data needed before the viewport render has been extracted
        // into owned/copied values above. `cache` now has no long-lived borrows, so
        // the closures below can capture it directly and also call viewer_cache()
        // inside short-lived scopes.
        #[cfg(feature = "regex")]
        let pending_match_scroll_y: Option<f32> = if self.want_scroll_to_active_match {
            let vc = viewer_cache(cache, &source_id);
            let search_cache = &vc.search_cache;
            let precise_y = search_cache
                .active_match()
                .and_then(|i| search_cache.search_match_virtual_ys().get(i).copied())
                .filter(|&y| y > 0.0);
            if precise_y.is_some() {
                precise_y
            } else {
                search_cache
                    .active_search_range()
                    .map(|r| r.start)
                    .and_then(|start| {
                        search_cache.virtual_y_for_byte_offset(&vc.split_points, start)
                    })
            }
        } else {
            None
        };

        Self::create_scroll_area(scroll_id, scroll_options).show_viewport(ui, |ui, viewport| {
            if let Some(y) = pending_scroll_y {
                // heading_y_positions stores virtual y (content-relative, 0 = top).
                // Inside show_viewport, next_widget_position().y = screen_top − scroll.
                // scroll_to_rect with Align::TOP sets new_scroll = y. ✓
                let r = egui::Rect::from_min_size(
                    egui::pos2(0.0, ui.next_widget_position().y + y),
                    egui::Vec2::ZERO,
                );
                ui.scroll_to_rect(r, Some(egui::Align::TOP));
            }

            #[cfg(feature = "regex")]
            Self::scroll_with_animation(ui, viewport, pending_match_scroll_y);

            if pending_delta != egui::Vec2::ZERO {
                ui.scroll_with_delta(pending_delta);
            }

            ui.set_height(page_size.y);
            let layout = egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true);

            let max_width = options.max_width(ui);
            ui.allocate_ui_with_layout(egui::vec2(max_width, 0.0), layout, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;

                // Compute the slice parameters and release the viewer_cache borrow
                // before the push_id closure so that `cache` is freely accessible
                // inside it.
                let viewport_height = viewport.max.y - viewport.min.y;
                let render_below = viewport.max.y + viewport_height;
                let (skip_height, skip_count, take_count) = {
                    let vc = viewer_cache(cache, &source_id);
                    #[cfg(feature = "regex")]
                    {
                        let search_cache = &mut vc.search_cache;
                        search_cache.last_viewport_top_y = viewport.min.y;
                        search_cache.last_viewport_height = viewport_height;
                    }
                    let preceding_split = vc
                        .split_points
                        .iter()
                        .rfind(|sp| sp.vend.y < viewport.min.y)
                        .cloned();
                    let first_vend = preceding_split.as_ref().map_or(Pos2::ZERO, |sp| sp.vend);
                    let last_event_index = vc
                        .split_points
                        .iter()
                        .find(|sp| sp.vstart.y > render_below)
                        .map_or(num_rows, |sp| sp.event_index);
                    let skip_height = first_vend.y.max(0.0);
                    // When a preceding split was found, its End(Block) is already
                    // accounted for in skip_height — re-processing it would add a
                    // duplicate newline. Start from the next event instead.
                    let (skip_count, take_count) = if let Some(sp) = preceding_split {
                        self.line.should_not_start_newline_forced = false;
                        // last_event_index should always be >= event_index because
                        // split-points are ordered, but guard against stale
                        // cache or tiny documents producing an underflow.
                        let take = last_event_index.saturating_sub(sp.event_index);
                        (sp.event_index + 1, take)
                    } else {
                        (0, last_event_index)
                    };
                    (skip_height, skip_count, take_count)
                }; // viewer_cache borrow released here

                // Set `content_origin_y` to the screen Y of virtual-Y = 0 (the
                // document top) for this frame. This makes match Ys recorded
                // by `event_text` comparable with `viewport.min.y` (the virtual
                // scroll offset). Matches in the rendered slice get their
                // exact pixel Y; those outside get the default 0.0 and are
                // treated as not-in-viewport by `sync_scrollable_active_match`.
                self.content_origin_y = ui.clip_rect().min.y - viewport.min.y;
                self.search_match_ys_scratch.clear();

                let mut events = events
                    .into_iter()
                    .enumerate()
                    .skip(skip_count)
                    .take(take_count)
                    .peekable();

                // Give the viewport render a distinct widget parent_id namespace
                // so that egui's warn_if_rect_changes_id check never fires.
                //
                // The check fires when the same screen rect has different widget
                // IDs *and* at least one widget shares a parent_id between
                // consecutive frames.  Widget IDs within this push_id scope are
                // counter-based (sequential from 0 each frame).  The counter
                // resets at the start of each rendered slice, so when skip_count
                // advances by 1 (viewport crosses a split-point boundary), every
                // widget in the visible overlap shifts its ID by 1 — same rect,
                // same parent_id, different ID → warning fires.
                //
                // Fix: bake skip_count into the push_id salt.  Consecutive frames
                // with different skip_count values get different parent_ids, so the
                // parent_id match condition is never met.  When skip_count is
                // stable (viewport moves within a split-point interval), the same
                // slice renders with identical counter values → same IDs → no
                // warning then either.
                //
                // The salt tuple also differs from the full-render path's implicit
                // parent (no push_id), so the transition-frame guard from the
                // full→viewport fix remains intact.
                //
                // IMPORTANT: push_id must be called BEFORE allocate_space so that
                // the cursor is still at (0, 0) — the left edge of a full-width
                // row.  Calling it after allocate_space leaves the cursor at
                // (max_width, …) (right edge), making available_rect_before_wrap()
                // return near-zero width and collapsing all content to a thin strip.
                ui.push_id(("__cm_viewport", skip_count), |ui| {
                    // Skip over off-screen content by reserving its vertical space.
                    // Full width is essential: a narrower allocation would leave
                    // the cursor mid-row, misaligning the first visible block.
                    ui.allocate_space(egui::vec2(max_width, skip_height));

                    // If this pass will be discarded (blind scroll toward an
                    // off-screen search match), skip expensive widget rendering
                    // entirely. The space allocation above is still needed so
                    // that egui has the correct total height for scroll
                    // calculations. `want_scroll_to_active_match` stays true,
                    // so `retry_scroll_to_active_match` below re-arms the flag
                    // for pass 2, which renders normally at the new offset.
                    if !ui.ctx().will_discard() {
                        while let Some((i, (e, src_span))) = events.next() {
                            if events.peek().is_none() {
                                self.line.should_end_newline_forced = false;
                            }
                            self.process_event(
                                ui,
                                &mut events,
                                e,
                                src_span,
                                cache,
                                options,
                                max_width,
                            );
                            if i == 0 {
                                self.line.should_not_start_newline_forced = false;
                            }
                        }

                        // Mirror `show()`'s deferred flush so that clicking a #fragment
                        // link while in the viewport path triggers a scroll next frame.
                        // Borrow vc fresh here: `cache` is free now that the
                        // process_event loop above has finished using it.
                        let vc = viewer_cache(cache, &source_id);
                        *vc.scroll_to_id_target_mut() = self.deferred_scroll_to_heading.take();

                        // Flush the per-match virtual-Y positions collected by
                        // `event_text` into the cache, exactly as the non-scrollable
                        // show() path does. Skipped on discarded frames (blind scroll
                        // toward an off-screen match) since no widgets rendered.
                        #[cfg(feature = "regex")]
                        vc.search_cache.update_show_viewport(
                            self.search_match_ys_scratch.drain(..),
                            viewport.min.y,
                            viewport_height,
                        );
                    }
                });
            });
        });

        // If any image in this render reported zero height, split points are stale.
        // Discard them so the next frame falls back to a full render.
        if self.any_image_loading {
            let vc = viewer_cache(cache, &source_id);
            vc.page_size = None;
            vc.split_points.clear();
            vc.heading_y_positions.clear();
        }

        // The active match wasn't inside the slice we just rendered. Re-arm
        // the request (bounded — see `retry_scroll_to_active_match`); the
        // blind scroll toward its approximate position (recomputed fresh
        // next frame from `pending_match_scroll_y`) should bring it into a
        // rendered slice within a frame or two, entirely within the cheap
        // viewport-only path. We only ever fall back to a full render if
        // there are no split points at all to approximate a position from
        // (e.g. a document with no top-level paragraphs/headings/code
        // blocks), which is the same data a full render would need to
        // populate anyway.
        #[cfg(feature = "regex")]
        if self.want_scroll_to_active_match {
            let vc = viewer_cache(cache, &source_id);
            if vc.search_cache.retry_scroll_to_active_match() && vc.split_points.is_empty() {
                vc.page_size = None;
            }
        }

        // Invalidate the cache when the available *width* changes (e.g. window resize).
        // Height changes — such as the search bar or TOC panel opening/closing —
        // do not affect split points or `page_size`: text wrapping only depends on
        // width, so a height-only change must never force a full render, especially on
        // a large document.
        {
            let vc = viewer_cache(cache, &source_id);
            let width_changed = (available_size.x - vc.available_size.x).abs() > 0.5;
            vc.available_size = available_size; // always keep Y bookkeeping current
            if width_changed {
                vc.page_size = None;
                vc.split_points.clear();
                vc.heading_y_positions.clear();
            }
        }
    }

    #[cfg(feature = "regex")]
    fn scroll_with_animation(ui: &Ui, viewport: egui::Rect, pending_match_scroll_y: Option<f32>) {
        if let Some(y) = pending_match_scroll_y
            && (y < viewport.min.y || y > viewport.max.y)
        {
            // The match's approximate block isn't in view yet.
            // Scroll to its approximate position immediately (no
            // animation) and request a discard so this pass is never
            // shown to the user. In pass 2 the scroll area begins with
            // the offset already committed, so the viewport lands on
            // the target; the per-widget code below then finds the
            // exact match and refines the scroll precisely.
            //
            // Using ScrollAnimation::none() is essential: the
            // animated default takes 0.1–0.3 s, but consecutive
            // passes within the same run_dyn loop share nearly the
            // same timestamp, so the animation would not progress and
            // pass 2's viewport would still be at the old position.
            let r = egui::Rect::from_min_size(
                egui::pos2(0.0, ui.next_widget_position().y + y),
                egui::Vec2::ZERO,
            );
            ui.scroll_to_rect_animation(
                r,
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
            ui.ctx().request_discard("scroll to active search match");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_event<'e>(
        &mut self,
        ui: &mut Ui,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        event: pulldown_cmark::Event,
        src_span: Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        let prev_end_para = std::mem::replace(&mut self.last_end_para, matches!(event, pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Paragraph)));
        if self.try_galley(ui, events, &event, &src_span, cache, options, max_width, prev_end_para) {
            return;
        }
        self.event(ui, event, src_span, cache, options, max_width);

        self.def_list_def_wrapping(events, max_width, cache, options, ui);
        self.item_list_wrapping(events, max_width, cache, options, ui);
        self.table(events, cache, options, ui, max_width);
        self.blockquote(events, max_width, cache, options, ui);
    }

    /// Kladd: ritar ett stycke eller en rubrik som ett enda galleri när läsläget för EPUB vill det (justerad text,
    /// förlagets blockstil eller markeringar i blocket). Ger sant om blocket är ritat.
    #[allow(clippy::too_many_arguments)]
    fn try_galley<'e>(
        &mut self,
        ui: &mut Ui,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        event: &pulldown_cmark::Event,
        src_span: &Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
        prev_end_para: bool,
    ) -> bool {
        let (heading, is_para) = match event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Paragraph) => (None, true),
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Heading { level, .. }) => (Some(*level), false),
            _ => return false,
        };
        if self.list.is_inside_a_list()
            || self.is_blockquote
            || self.is_table
            || self.link.is_some()
            || self.image.is_some()
            || self.code_block.is_some()
            || self.def_list.is_def_list_def
        {
            return false;
        }
        let Some(extras) = current_extras(ui) else { return false };
        let style = extras.blocks.get(&src_span.start).cloned();
        if style.is_none() && !(is_para && extras.justify) && !extras.marks_in(src_span) {
            return false;
        }
        let style = style.unwrap_or_default();
        let evs = delayed_events(events, |end| {
            matches!(end, pulldown_cmark::TagEnd::Paragraph | pulldown_cmark::TagEnd::Heading(_))
        });
        if self.galley_block(ui, heading, &evs, src_span, cache, options, &extras, &style, prev_end_para) {
            self.last_end_para = is_para;
            return true;
        }
        // Något i blocket går inte att lägga i ett galleri (bild, formel …): rita det på vanligt sätt.
        self.event(ui, event.clone(), src_span.clone(), cache, options, max_width);
        let mut it = evs.into_iter().enumerate().peekable();
        while let Some((_, (e, sp))) = it.next() {
            self.process_event(ui, &mut it, e, sp, cache, options, max_width);
        }
        true
    }

    /// Kladd: den aktuella textstorleken (brödtext eller rubrik) för små versaler.
    fn current_size(&self, ui: &Ui) -> f32 {
        let body = ui.style().text_styles.get(&TextStyle::Body).map_or(14.0, |d| d.size);
        let Some(level) = self.text_style.heading else { return body };
        let max = ui.style().text_styles.get(&TextStyle::Heading).map_or(32.0, |d| d.size);
        let diff = max - body;
        match level {
            0 => max,
            1 => body + diff * 0.668,
            2 => body + diff * 0.501,
            3 => body + diff * 0.334,
            4 => body + diff * 0.167,
            _ => body + diff * 0.167,
        }
    }

    /// Kladd: lägger en textbit (med nuvarande stil) i galleriet. Med `small_caps` ritas gemener som
    /// versaler i mindre storlek (byte-längden är oförändrad så att källpositionerna stämmer).
    #[allow(clippy::too_many_arguments)]
    fn galley_push(
        &self,
        ui: &Ui,
        job: &mut LayoutJob,
        pieces: &mut Vec<(Range<usize>, Range<usize>)>,
        text: &str,
        span: &Range<usize>,
        small_caps: bool,
    ) {
        if text.is_empty() {
            return;
        }
        let valign = ui.text_valign();
        let mut add = |job: &mut LayoutJob, t: &str, lower: bool, off: usize| {
            if t.is_empty() {
                return;
            }
            let mut rich = self.text_style.to_richtext(ui, t);
            if lower {
                rich = rich.size(self.current_size(ui) * 0.8);
            }
            let from = job.text.len();
            rich.append_to(job, ui.style(), egui::FontSelection::Default, valign);
            let start = span.start.saturating_add(off).min(span.end.max(span.start));
            pieces.push((from..job.text.len(), start..span.end.max(start)));
        };
        if !small_caps {
            add(job, text, false, 0);
            return;
        }
        let mut run = String::new();
        let mut run_lower = false;
        let mut off = 0;
        let mut run_off = 0;
        for ch in text.chars() {
            let lower = ch.is_lowercase();
            if !run.is_empty() && lower != run_lower {
                let t = if run_lower { upper_same_len(&run) } else { run.clone() };
                add(job, &t, run_lower, run_off);
                run.clear();
                run_off = off;
            }
            if run.is_empty() {
                run_off = off;
                run_lower = lower;
            }
            run.push(ch);
            off += ch.len_utf8();
        }
        if !run.is_empty() {
            let t = if run_lower { upper_same_len(&run) } else { run };
            add(job, &t, run_lower, run_off);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn galley_block(
        &mut self,
        ui: &mut Ui,
        heading: Option<HeadingLevel>,
        evs: &[(pulldown_cmark::Event<'_>, Range<usize>)],
        block_span: &Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        extras: &ViewExtras,
        style: &BlockStyle,
        prev_end_para: bool,
    ) -> bool {
        use pulldown_cmark::{Event, Tag, TagEnd};
        // Går allt att lägga i ett galleri?
        let simple = evs.iter().all(|(e, _)| {
            matches!(
                e,
                Event::Text(_)
                    | Event::Code(_)
                    | Event::InlineHtml(_)
                    | Event::SoftBreak
                    | Event::HardBreak
                    | Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Superscript | Tag::Subscript)
                    | Event::End(
                        TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Superscript | TagEnd::Subscript | TagEnd::Paragraph | TagEnd::Heading(_)
                    )
            )
        });
        if !simple {
            return false;
        }

        let level_idx = heading.map(|l| match l {
            HeadingLevel::H1 => 0,
            HeadingLevel::H2 => 1,
            HeadingLevel::H3 => 2,
            HeadingLevel::H4 => 3,
            HeadingLevel::H5 => 4,
            HeadingLevel::H6 => 5,
        });
        let body = ui.style().text_styles.get(&TextStyle::Body).map_or(14.0, |d| d.size);
        let tight = style.indent > 0.0 && prev_end_para && heading.is_none();
        if heading.is_some() {
            newline(ui);
            self.text_style.heading = level_idx;
        } else if !tight {
            self.line.try_insert_start(ui);
        }

        let mut job = LayoutJob::default();
        let mut pieces: Vec<(Range<usize>, Range<usize>)> = Vec::new();
        let mut links: Vec<(Range<usize>, String)> = Vec::new();
        let mut cur_link: Option<(usize, String)> = None;
        for (e, span) in evs {
            match e {
                Event::Text(t) | Event::InlineHtml(t) => self.galley_push(ui, &mut job, &mut pieces, t, span, style.small_caps),
                Event::Code(t) => {
                    self.text_style.code = true;
                    self.galley_push(ui, &mut job, &mut pieces, t, span, false);
                    self.text_style.code = false;
                }
                Event::SoftBreak => {
                    let at = span.start..span.start;
                    self.galley_push(ui, &mut job, &mut pieces, " ", &at, false);
                }
                Event::HardBreak => {
                    let at = span.start..span.start;
                    self.galley_push(ui, &mut job, &mut pieces, "\n", &at, false);
                }
                Event::Start(Tag::Emphasis) => self.text_style.emphasis = true,
                Event::Start(Tag::Strong) => self.text_style.strong = true,
                Event::Start(Tag::Strikethrough) => self.text_style.strikethrough = true,
                Event::End(TagEnd::Emphasis) => self.text_style.emphasis = false,
                Event::End(TagEnd::Strong) => self.text_style.strong = false,
                Event::End(TagEnd::Strikethrough) => self.text_style.strikethrough = false,
                Event::Start(Tag::Link { dest_url, .. }) => cur_link = Some((job.text.len(), dest_url.to_string())),
                Event::End(TagEnd::Link) => {
                    if let Some((from, dest)) = cur_link.take() {
                        if from < job.text.len() {
                            links.push((from..job.text.len(), dest));
                        }
                    }
                }
                _ => {}
            }
        }
        if heading.is_some() {
            self.text_style.heading = None;
        }
        // Länkar: länkfärg (som den vanliga etikettvägen; ingen ständig understrykning, den hamnar långt under raden).
        let link_color = ui.visuals().hyperlink_color;
        let link_ranges: Vec<(Range<usize>, ())> = links.iter().map(|(r, _)| (r.clone(), ())).collect();
        egui_commonmark_backend::search::format_ranges(&mut job, &link_ranges, |f, _| {
            f.color = link_color;
        });
        // Markeringar och sökträffar som bakgrunder.
        let mut mark_hits: Vec<(usize, Range<usize>)> = Vec::new();
        let mut mark_colors: Vec<(Range<usize>, egui::Color32)> = Vec::new();
        for (jr, sr) in &pieces {
            let len = jr.end - jr.start;
            for (mi, m) in extras.marks.iter().enumerate() {
                if m.range.start < sr.end.max(sr.start + len) && m.range.end > sr.start {
                    let a = m.range.start.saturating_sub(sr.start).min(len);
                    let b = m.range.end.saturating_sub(sr.start).min(len);
                    if a < b {
                        mark_hits.push((mi, jr.start + a..jr.start + b));
                        mark_colors.push((jr.start + a..jr.start + b, m.color));
                    }
                }
            }
        }
        egui_commonmark_backend::search::apply_mark_colors(&mut job, &mark_colors);
        #[cfg(feature = "regex")]
        let mut search_ivs: Vec<(Range<usize>, bool)> = Vec::new();
        #[cfg(feature = "regex")]
        let mut search_global: Vec<usize> = Vec::new();
        #[cfg(feature = "regex")]
        {
            let vc = viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL));
            let sc = &vc.search_cache;
            let ranges = sc.search_ranges();
            if !ranges.is_empty() {
                for (jr, sr) in &pieces {
                    let len = jr.end - jr.start;
                    let local = search_intervals(ranges, sc.active_search_range(), sr, len);
                    let idxs: Vec<usize> = ranges
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.start < r.end && r.start < sr.end && r.end > sr.start)
                        .map(|(i, _)| i)
                        .collect();
                    for (k, (r, active)) in local.into_iter().enumerate() {
                        search_ivs.push((jr.start + r.start..jr.start + r.end, active));
                        search_global.push(idxs.get(k).copied().unwrap_or(0));
                    }
                }
                search_ivs.sort_by_key(|(r, _)| r.start);
            }
        }
        #[cfg(feature = "regex")]
        if !search_ivs.is_empty() {
            egui_commonmark_backend::search::apply_search_highlights(
                &mut job,
                &search_ivs,
                options.search_match_bg(ui),
                options.search_active_match_bg(ui),
            );
        }

        // Layout.
        let align = style.align.unwrap_or(if heading.is_none() && extras.justify { BlockAlign::Justify } else { BlockAlign::Left });
        let avail = ui.available_width().max(80.0);
        let ml = (style.margin_left * body).clamp(0.0, avail * 0.6);
        let text_w = (avail - ml).max(60.0);
        if style.indent > 0.0 && matches!(align, BlockAlign::Left | BlockAlign::Justify) {
            if let Some(first) = job.sections.first_mut() {
                first.leading_space = style.indent * body;
            }
        }
        job.wrap.max_width = text_w;
        job.halign = match align {
            BlockAlign::Center => egui::Align::Center,
            BlockAlign::Right => egui::Align::RIGHT,
            _ => egui::Align::LEFT,
        };
        job.justify = false;
        if align == BlockAlign::Justify {
            // egui:s egen justering (`LayoutJob::justify`) tar bort radens inledande blanksteg och därmed indraget
            // på första raden, så mellanrummen fördelas här i stället.
            job = justify_job(ui, job, text_w);
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let selectable = ui.style().interaction.selectable_labels;
        let mut sense = egui::Sense::click();
        if selectable {
            sense = egui::Sense::click_and_drag();
            sense -= egui::Sense::FOCUSABLE;
        }
        let (rect, response) = ui.allocate_exact_size(egui::vec2(avail, galley.size().y.max(1.0)), sense);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), galley.text()));
        let left = rect.left() + ml;
        let pos = match align {
            BlockAlign::Center => egui::pos2(left + text_w / 2.0, rect.top()),
            BlockAlign::Right => egui::pos2(rect.right(), rect.top()),
            _ => egui::pos2(left, rect.top()),
        };

        // Sökträffarnas lägen (för rullning och aktuell träff).
        #[cfg(feature = "regex")]
        if !search_ivs.is_empty() {
            let content_origin_y = self.content_origin_y;
            for (i, (r, active)) in search_ivs.iter().enumerate() {
                if let Some(rc) = highlight_rect_for_byte_range(&galley, pos, r.clone()) {
                    self.search_match_ys_scratch.push((search_global.get(i).copied().unwrap_or(0), rc.min.y - content_origin_y));
                    if *active && self.want_scroll_to_active_match {
                        ui.scroll_to_rect(rc, Some(egui::Align::Center));
                        self.want_scroll_to_active_match = false;
                    }
                }
            }
        }

        if ui.is_rect_visible(rect) {
            let color = ui.style().visuals.text_color();
            if selectable {
                egui::text_selection::LabelSelectionState::label_text_selection(ui, &response, pos, galley.clone(), color, egui::Stroke::NONE);
            } else {
                ui.painter().add(egui::epaint::TextShape::new(pos, galley.clone(), color));
            }
            // Anteckningsmärke efter varje markering som har en anteckning.
            for (mi, m) in extras.marks.iter().enumerate() {
                if !m.note || !block_span.contains(&m.range.end.saturating_sub(1)) {
                    continue;
                }
                if let Some((_, jr)) = mark_hits.iter().filter(|(i, _)| *i == mi).max_by_key(|(_, r)| r.end) {
                    if let Some(rc) = highlight_rect_for_byte_range(&galley, pos, jr.clone()) {
                        let c = egui::pos2(rc.right() + 5.0, rc.top() + 5.0);
                        ui.painter().circle_filled(c, 3.5, extras.note_color);
                    }
                }
            }
        }

        // Klick: länk eller markering under pekaren.
        if let Some(ptr) = response.hover_pos().or_else(|| response.interact_pointer_pos()) {
            if let Some(ci) = galley_char_at(&galley, pos, ptr) {
                let text = &galley.job.text;
                let byte = text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
                let link = links.iter().find(|(r, _)| r.start <= byte && byte < r.end).map(|(_, d)| d.clone());
                let mark = mark_hits.iter().find(|(_, r)| r.start <= byte && byte < r.end).map(|(i, _)| *i);
                if link.is_some() || mark.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    if let Some(dest) = link {
                        self.galley_link(ui, cache, options, dest);
                    } else if let Some(mi) = mark {
                        put_mark_hit(ui.ctx(), (mi, ptr));
                    }
                }
            }
        }

        self.line.try_insert_end(ui);
        true
    }

    /// Kladd: klick på en länk i ett galleri (samma som `Link::end`).
    fn galley_link(&mut self, ui: &mut Ui, cache: &mut CommonMarkCache, options: &CommonMarkOptions, dest: String) {
        if cache.link_hooks().contains_key(&dest) {
            cache.link_hooks_mut().insert(dest, true);
        } else if options.enable_scroll_to_heading && dest.starts_with('#') {
            self.deferred_scroll_to_heading = Some(dest[1..].to_owned());
        } else {
            ui.ctx().open_url(egui::OpenUrl::same_tab(dest));
        }
    }

    fn def_list_def_wrapping<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.def_list.is_def_list_def {
            self.def_list.is_def_list_def = false;

            let item_events = delayed_events(events, |tag| {
                matches!(tag, pulldown_cmark::TagEnd::DefinitionListDefinition)
            });

            let mut events_iter = item_events.into_iter().enumerate().peekable();

            self.line.try_insert_start(ui);

            // Process a single event separately so that we do not insert spaces where we do not
            // want them
            self.line.should_start_newline = false;
            if let Some((_, (e, src_span))) = events_iter.next() {
                self.process_event(ui, &mut events_iter, e, src_span, cache, options, max_width);
            }

            ui.label(" ".repeat(options.indentation_spaces));
            self.line.should_start_newline = true;
            self.line.should_end_newline = false;
            // Required to ensure that the content is aligned with the indentation
            ui.horizontal_wrapped(|ui| {
                while let Some((_, (e, src_span))) = events_iter.next() {
                    self.process_event(
                        ui,
                        &mut events_iter,
                        e,
                        src_span,
                        cache,
                        options,
                        max_width,
                    );
                }
            });
            self.line.should_end_newline = true;

            // Only end the definition items line if it is not the last element in the list
            if !matches!(
                events.peek(),
                Some((
                    _,
                    (
                        pulldown_cmark::Event::End(pulldown_cmark::TagEnd::DefinitionList),
                        _
                    )
                ))
            ) {
                self.line.try_insert_end(ui);
            }
        }
    }

    fn item_list_wrapping<'e>(
        &mut self,
        events: &mut impl Iterator<Item = EventIteratorItem<'e>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.is_list_item {
            self.is_list_item = false;

            let item_events = delayed_events_list_item(events);
            let mut events_iter = item_events.into_iter().enumerate().peekable();

            // Required to ensure that the content of the list item is aligned with
            // the * or - when wrapping
            ui.horizontal_wrapped(|ui| {
                while let Some((_, (e, src_span))) = events_iter.next() {
                    self.process_event(
                        ui,
                        &mut events_iter,
                        e,
                        src_span,
                        cache,
                        options,
                        max_width,
                    );
                }
            });
        }
    }

    fn blockquote<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.is_blockquote {
            let mut collected_events = delayed_events(events, |tag| {
                matches!(tag, pulldown_cmark::TagEnd::BlockQuote(_))
            });
            self.line.try_insert_start(ui);

            // Currently the blockquotes are made in such a way that they need a newline at the end
            // and the start so when this is the first element in the markdown the newline must be
            // manually enabled
            self.line.should_not_start_newline_forced = false;

            // Capture the source byte range of the first Text event *before*
            // `parse_alerts` deletes it.  This is the position of the alert keyword
            // (e.g. `[!NOTE]`) in the source; `update_search_matches` uses the same
            // range as the anchor for title matches, so the renderer can identify
            // them here and apply `label_with_search_highlight` to the title label.
            #[cfg(feature = "regex")]
            let identifier_src_range: Range<usize> = collected_events
                .iter()
                .find(|(e, _)| matches!(e, pulldown_cmark::Event::Text(_)))
                .map(|(_, r)| r.clone())
                .unwrap_or(0..0);

            if let Some(alert) = parse_alerts(&options.alerts, &mut collected_events) {
                #[cfg(feature = "regex")]
                let ident_src = identifier_src_range;

                blockquote(ui, alert.accent_color, |ui| {
                    newline(ui);
                    ui.colored_label(alert.accent_color, alert.icon.to_string());
                    ui.add_space(3.0);

                    // Render the alert title with search highlighting when the active
                    // query matched `identifier_rendered` (e.g. "Note" for `[!NOTE]`).
                    // `update_search_matches` stored the keyword's source bytes as the
                    // match range, so we check overlap with `ident_src` to decide.
                    #[cfg(feature = "regex")]
                    let (has_title_match, is_title_active, title_global_idx) = {
                        let vc = viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL));
                        let search_cache = &vc.search_cache;
                        let ranges = search_cache.search_ranges();
                        let has_match = !ranges.is_empty()
                            && ranges
                                .iter()
                                .any(|r| r.start < ident_src.end && r.end > ident_src.start);
                        let is_active = has_match
                            && search_cache.active_search_range().is_some_and(|a| {
                                a.start < ident_src.end && a.end > ident_src.start
                            });
                        let global_idx = if has_match {
                            ranges
                                .iter()
                                .enumerate()
                                .find(|(_, r)| r.start < ident_src.end && r.end > ident_src.start)
                                .map(|(i, _)| i)
                        } else {
                            None
                        };
                        (has_match, is_active, global_idx)
                    }; // all immutable borrows of cache released here

                    #[cfg(not(feature = "regex"))]
                    let (has_title_match, _is_title_active, _title_global_idx): (
                        bool,
                        bool,
                        Option<usize>,
                    ) = (false, false, None);

                    if has_title_match {
                        #[cfg(feature = "regex")]
                        {
                            let title_len = alert.identifier_rendered.len();
                            let intervals = vec![(0..title_len, is_title_active)];
                            let (_, active_rect, all_rects) = label_with_search_highlight(
                                ui,
                                RichText::new(&alert.identifier_rendered).color(alert.accent_color),
                                &intervals,
                                options.search_match_bg(ui),
                                options.search_active_match_bg(ui),
                            );
                            if let Some(idx) = title_global_idx
                                && let Some(Some(rect)) = all_rects.first()
                            {
                                self.search_match_ys_scratch
                                    .push((idx, rect.min.y - self.content_origin_y));
                            }
                            if self.want_scroll_to_active_match
                                && let Some(rect) = active_rect
                            {
                                ui.scroll_to_rect(rect, Some(egui::Align::Center));
                                self.want_scroll_to_active_match = false;
                            }
                        }
                    } else {
                        ui.colored_label(alert.accent_color, &alert.identifier_rendered);
                    }

                    newline(ui);
                    for (event, src_span) in collected_events {
                        self.event(ui, event, src_span, cache, options, max_width);
                    }
                });
            } else {
                blockquote(ui, ui.visuals().weak_text_color(), |ui| {
                    self.text_style.quote = true;
                    for (event, src_span) in collected_events {
                        self.event(ui, event, src_span, cache, options, max_width);
                    }
                    self.text_style.quote = false;
                });
            }

            if events.peek().is_none() {
                self.line.should_end_newline_forced = false;
            }

            self.line.try_insert_end(ui);
            self.is_blockquote = false;
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn table<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
        max_width: f32,
    ) {
        if self.is_table {
            self.line.try_insert_start(ui);

            let id = ui.make_persistent_id(("_table", self.curr_table));
            self.curr_table += 1;

            // egui's `ScrollArea` consumes `scroll_to_rect` calls for BOTH dimensions
            // (see egui source: "We always take both scroll targets regardless of
            // which scroll axes are enabled").  This means `scroll_to_rect` called
            // from `event_text()` inside the table's horizontal scroll area silently
            // discards the vertical component, preventing the outer vertical scroll
            // area from bringing the row into view.  Snapshot the scroll-request
            // state before the table renders; if the table consumed it we re-issue
            // on the outer `ui` afterwards.
            #[cfg(feature = "regex")]
            let want_scroll_before = self.want_scroll_to_active_match;
            #[cfg(feature = "regex")]
            let scratch_start = self.search_match_ys_scratch.len();

            // Kladd: ramar och linjer, fetstil i rubrikraden, luft i cellerna och radbrytning.
            let mut row_points: Vec<(usize, f32)> = Vec::new();
            let content_origin_y = self.content_origin_y;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                let Table { header, rows } = parse_table(events);

                ui.spacing_mut().scroll.content_margin.bottom = ui.spacing().scroll.bar_width as i8;

                let ncols = rows.iter().chain(std::iter::once(&header)).map(Vec::len).max().unwrap_or(0);
                let mut chars = vec![3usize; ncols];
                for row in rows.iter().chain(std::iter::once(&header)) {
                    for (c, col) in row.iter().enumerate() {
                        let n: usize = col
                            .iter()
                            .map(|(e, _)| match e {
                                pulldown_cmark::Event::Text(t) | pulldown_cmark::Event::Code(t) => {
                                    t.chars().count()
                                }
                                _ => 0,
                            })
                            .sum();
                        chars[c] = chars[c].max(n);
                    }
                }
                let char_w = ui.text_style_height(&TextStyle::Body) * 0.56;
                let widths = table_column_widths(&chars, ui.available_width() - 20.0, char_w);

                let text_color = ui.visuals().text_color();
                let line = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
                let head_line = egui::Stroke::new(1.5, text_color.gamma_multiply(0.55));
                let mut cell_rects: Vec<Vec<egui::Rect>> = Vec::new();

                egui::ScrollArea::horizontal()
                    .id_salt(id.with("_hscroll"))
                    .show(ui, |ui| {
                        egui::Grid::new(id)
                            .spacing(egui::vec2(0.0, 0.0))
                            .with_row_color(|row, style| {
                                let faint = style.visuals.faint_bg_color;
                                if row == 0 {
                                    Some(faint.lerp_to_gamma(style.visuals.text_color(), 0.09))
                                } else if row % 2 == 0 {
                                    Some(faint)
                                } else {
                                    None
                                }
                            })
                            .show(ui, |ui| {
                                for (ri, row) in std::iter::once(header).chain(rows).enumerate() {
                                    let mut rects = Vec::new();
                                    let row_start = row.first().and_then(|c| c.first()).map(|(_, s)| s.start);
                                    for (ci, col) in row.into_iter().enumerate() {
                                        let w = widths.get(ci).copied().unwrap_or(120.0);
                                        let r = ui.scope(|ui| {
                                            ui.set_max_width(w);
                                            egui::Frame::NONE
                                                .inner_margin(egui::Margin::symmetric(9, 5))
                                                .show(ui, |ui| {
                                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true), |ui| {
                                                        for (e, src_span) in col {
                                                            let tmp_start = std::mem::replace(
                                                                &mut self.line.should_start_newline,
                                                                false,
                                                            );
                                                            let tmp_end = std::mem::replace(
                                                                &mut self.line.should_end_newline,
                                                                false,
                                                            );
                                                            if ri == 0 {
                                                                self.text_style.strong = true;
                                                            }
                                                            self.event(ui, e, src_span, cache, options, max_width);
                                                            self.line.should_start_newline = tmp_start;
                                                            self.line.should_end_newline = tmp_end;
                                                        }
                                                    });
                                                })
                                                .response
                                                .rect
                                        });
                                        if ri == 0 {
                                            self.text_style.strong = false;
                                        }
                                        rects.push(r.inner);
                                    }
                                    if let (Some(start), Some(first)) = (row_start, rects.first()) {
                                        row_points.push((start, first.top() - content_origin_y));
                                    }
                                    cell_rects.push(rects);
                                    ui.end_row();
                                }
                            });
                    });

                // Linjer mellan raderna och kolumnerna, och en kraftigare under rubrikraden.
                let area = ui.min_rect();
                let painter = ui.painter_at(area.expand(2.0));
                if let Some(first) = cell_rects.first() {
                    for r in first.iter().skip(1) {
                        painter.vline(r.left(), (first[0].top())..=area.bottom(), line);
                    }
                }
                for (ri, row) in cell_rects.iter().enumerate().skip(1) {
                    if let Some(r) = row.first() {
                        painter.hline(area.x_range(), r.top(), if ri == 1 { head_line } else { line });
                    }
                }
            });
            viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL)).line_points.extend(row_points);

            // If the active-match scroll was satisfied inside the table, the
            // `scroll_to_rect` call from `event_text()` was intercepted by the
            // horizontal scroll area (see comment above).  Re-issue it on the
            // outer `ui` using the virtual Y we recorded during the table render
            // so the outer vertical scroll area can bring the row into view.
            #[cfg(feature = "regex")]
            if want_scroll_before
                && !self.want_scroll_to_active_match
                && let Some(active_idx) =
                    viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL))
                        .search_cache
                        .active_match()
            {
                let match_virtual_y = self.search_match_ys_scratch[scratch_start..]
                    .iter()
                    .find(|(gi, _)| *gi == active_idx)
                    .map(|(_, y)| *y);
                if let Some(vy) = match_virtual_y {
                    let screen_y = self.content_origin_y + vy;
                    let line_height = ui.text_style_height(&TextStyle::Body);
                    let target_rect = egui::Rect::from_min_size(
                        egui::pos2(0.0, screen_y),
                        egui::vec2(1.0, line_height),
                    );
                    ui.scroll_to_rect(target_rect, Some(egui::Align::Center));
                }
            }

            self.is_table = false;
            if events.peek().is_none() {
                self.line.should_end_newline_forced = false;
            }

            self.line.try_insert_end(ui);
        }
    }

    fn event(
        &mut self,
        ui: &mut Ui,
        event: pulldown_cmark::Event,
        src_span: Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        match event {
            pulldown_cmark::Event::Start(tag) => self.start_tag(ui, tag, cache, options),
            pulldown_cmark::Event::End(tag) => self.end_tag(ui, tag, cache, options, max_width),
            pulldown_cmark::Event::Text(text) => {
                self.event_text(text, src_span, ui, cache, options);
            }
            pulldown_cmark::Event::Code(text) => {
                self.text_style.code = true;
                self.event_text(text, src_span, ui, cache, options);
                self.text_style.code = false;
            }
            pulldown_cmark::Event::InlineHtml(text) => {
                self.event_text(text, src_span, ui, cache, options);
            }

            pulldown_cmark::Event::Html(text) => {
                if options.html_fn.is_some() {
                    self.html_block.push_str(&text);
                } else {
                    self.event_text(text, src_span, ui, cache, options);
                }
            }
            pulldown_cmark::Event::FootnoteReference(footnote) => {
                footnote_start(ui, &footnote);
            }
            pulldown_cmark::Event::SoftBreak => {
                soft_break(ui);
            }
            pulldown_cmark::Event::HardBreak => newline(ui),
            pulldown_cmark::Event::Rule => {
                self.line.try_insert_start(ui);
                rule(ui, self.line.can_insert_end());
            }
            pulldown_cmark::Event::TaskListMarker(mut checkbox) => {
                if options.mutable {
                    if ui
                        .add(egui::Checkbox::without_text(&mut checkbox))
                        .clicked()
                    {
                        self.checkbox_events.push(CheckboxClickEvent {
                            checked: checkbox,
                            span: src_span,
                        });
                    }
                } else {
                    ui.add(ImmutableCheckbox::without_text(&mut checkbox));
                }
            }
            pulldown_cmark::Event::InlineMath(tex) => {
                if let Some(math_fn) = options.math_fn {
                    math_fn(ui, &tex, true);
                }
            }
            pulldown_cmark::Event::DisplayMath(tex) => {
                if let Some(math_fn) = options.math_fn {
                    math_fn(ui, &tex, false);
                }
            }
        }
    }

    #[cfg_attr(not(feature = "regex"), allow(unused_variables))]
    fn event_text(
        &mut self,
        text: CowStr,
        src_span: Range<usize>,
        ui: &mut Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
    ) {
        if let Some(image) = &mut self.image {
            image.push_alt_text(self.text_style.to_richtext(ui, &text), src_span);
        } else if let Some(block) = &mut self.code_block {
            block.push_text(&text, src_span);
        } else if let Some(link) = &mut self.link {
            link.push_text(self.text_style.to_richtext(ui, &text), src_span);
        } else {
            self.event_label(text, src_span, ui, cache, options);
        }
    }

    #[cfg(not(feature = "regex"))]
    #[allow(unused_variables)]
    fn event_label(
        &self,
        text: CowStr,
        src_span: Range<usize>,
        ui: &mut Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
    ) {
        let rich_text = self.text_style.to_richtext(ui, &text);
        ui.label(rich_text);
    }

    #[cfg(feature = "regex")]
    fn event_label(
        &mut self,
        text: CowStr,
        src_span: Range<usize>,
        ui: &mut Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
    ) {
        let rich_text = self.text_style.to_richtext(ui, &text);

        let vc = viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL));
        let search_cache = &mut vc.search_cache;
        let ranges = search_cache.search_ranges();
        if ranges.is_empty() {
            ui.label(rich_text);
            return;
        }

        let intervals = search_intervals(
            ranges,
            search_cache.active_search_range(),
            &src_span,
            text.len(),
        );
        let (_, active_rect, all_rects) = label_with_search_highlight(
            ui,
            rich_text,
            &intervals,
            options.search_match_bg(ui),
            options.search_active_match_bg(ui),
        );

        // Record the virtual-y (scroll-independent) position for each
        // global match that falls in this text run. `global_match_indices[j]`
        // corresponds to `intervals[j]` and `all_rects[j]`: both iterate
        // search_ranges in document order with the same filter, so the
        // j-th surviving entry is the same match in both.
        let content_origin_y = self.content_origin_y;
        let global_match_indices: Vec<usize> = search_cache
            .search_ranges()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.start < r.end && r.start < src_span.end && r.end > src_span.start)
            .map(|(i, _)| i)
            .collect();

        for (global_idx, maybe_rect) in global_match_indices.iter().zip(all_rects.iter()) {
            if let Some(rect) = maybe_rect {
                self.search_match_ys_scratch
                    .push((*global_idx, rect.min.y - content_origin_y));
            }
        }

        if self.want_scroll_to_active_match
            && let Some(rect) = active_rect
        {
            ui.scroll_to_rect(rect, Some(egui::Align::Center));
            self.want_scroll_to_active_match = false;
        }
    }

    fn start_tag(
        &mut self,
        ui: &mut Ui,
        tag: pulldown_cmark::Tag,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
    ) {
        let vc = viewer_cache(cache, &options.source_id.unwrap_or(Id::NULL));
        match tag {
            pulldown_cmark::Tag::Paragraph => {
                self.line.try_insert_start(ui);
            }
            pulldown_cmark::Tag::Heading { level, id, .. } => {
                if let Some(scroll_target) = vc.scroll_to_id_target()
                    && let Some(id) = id
                    && id.into_string() == scroll_target
                {
                    // Kladd: `scroll_to_cursor` använder mitten av det lediga utrymmet i höjdled i
                    // den här radbrytande layouten och hamnar en halv skärm för långt ned.
                    let at = ui.cursor().min;
                    ui.scroll_to_rect(egui::Rect::from_min_size(at, egui::vec2(1.0, 1.0)), Some(egui::Align::TOP));
                    vc.scroll_to_id_target_mut().take();
                }

                // Headings should always insert a newline even if it is at the start.
                // Whether this is okay in all scenarios is a different question.
                newline(ui);
                self.text_style.heading = Some(match level {
                    HeadingLevel::H1 => 0,
                    HeadingLevel::H2 => 1,
                    HeadingLevel::H3 => 2,
                    HeadingLevel::H4 => 3,
                    HeadingLevel::H5 => 4,
                    HeadingLevel::H6 => 5,
                });
            }

            // deliberately not using the built in alerts from pulldown-cmark as
            // the markdown itself cannot be localized :( e.g: [!TIP]
            pulldown_cmark::Tag::BlockQuote(_) => {
                self.is_blockquote = true;
            }
            pulldown_cmark::Tag::CodeBlock(c) => {
                match c {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                        self.code_block = Some(crate::CodeBlock {
                            lang: Some(lang.to_string()),
                            content: "".to_string(),
                            chunks: Vec::new(),
                        });
                    }
                    pulldown_cmark::CodeBlockKind::Indented => {
                        self.code_block = Some(crate::CodeBlock {
                            lang: None,
                            content: "".to_string(),
                            chunks: Vec::new(),
                        });
                    }
                }
                // A code block always needs a line of its own. Inside a list
                // `should_start_newline` is off, so `try_insert_start` would do
                // nothing and the block would be laid out after the item's text,
                // leaving it only the width remaining on that line.
                if !self.line.should_not_start_newline_forced {
                    newline(ui);
                }
            }

            pulldown_cmark::Tag::List(point) => {
                if !self.list.is_inside_a_list() && self.line.can_insert_start() {
                    newline(ui);
                }

                if let Some(number) = point {
                    self.list.start_level_with_number(number);
                } else {
                    self.list.start_level_without_number();
                }
                self.line.should_start_newline = false;
                self.line.should_end_newline = false;
            }

            pulldown_cmark::Tag::Item => {
                self.is_list_item = true;
                self.list.start_item(ui, options);
            }

            pulldown_cmark::Tag::FootnoteDefinition(note) => {
                self.line.try_insert_start(ui);

                self.line.should_start_newline = false;
                self.line.should_end_newline = false;
                footnote(ui, &note);
            }
            pulldown_cmark::Tag::Table(_) => {
                self.is_table = true;
            }
            pulldown_cmark::Tag::TableHead => {}
            pulldown_cmark::Tag::TableRow => {}
            pulldown_cmark::Tag::TableCell => {}
            pulldown_cmark::Tag::Emphasis => {
                self.text_style.emphasis = true;
            }
            pulldown_cmark::Tag::Strong => {
                self.text_style.strong = true;
            }
            pulldown_cmark::Tag::Strikethrough => {
                self.text_style.strikethrough = true;
            }
            pulldown_cmark::Tag::Link { dest_url, .. } => {
                self.link = Some(crate::Link {
                    destination: dest_url.to_string(),
                    ..Default::default()
                });
            }
            pulldown_cmark::Tag::Image { dest_url, .. } => {
                self.image = Some(crate::Image::new(&dest_url, options));
            }
            pulldown_cmark::Tag::HtmlBlock => {
                self.line.try_insert_start(ui);
            }
            pulldown_cmark::Tag::MetadataBlock(_) => {}

            pulldown_cmark::Tag::DefinitionList => {
                self.line.try_insert_start(ui);
                self.def_list.is_first_item = true;
            }
            pulldown_cmark::Tag::DefinitionListTitle => {
                // we disable newline as the first title should not insert a newline
                // as we have already done that upon the DefinitionList Tag
                if !self.def_list.is_first_item {
                    self.line.try_insert_start(ui)
                } else {
                    self.def_list.is_first_item = false;
                }
            }
            pulldown_cmark::Tag::DefinitionListDefinition => {
                self.def_list.is_def_list_def = true;
            }
            // Not yet supported
            pulldown_cmark::Tag::Superscript | pulldown_cmark::Tag::Subscript => {}
        }
    }

    fn end_tag(
        &mut self,
        ui: &mut Ui,
        tag: pulldown_cmark::TagEnd,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        match tag {
            pulldown_cmark::TagEnd::Paragraph => {
                self.line.try_insert_end(ui);
            }
            pulldown_cmark::TagEnd::Heading { .. } => {
                self.line.try_insert_end(ui);
                self.text_style.heading = None;
            }
            pulldown_cmark::TagEnd::BlockQuote(_) => {}
            pulldown_cmark::TagEnd::CodeBlock => {
                self.end_code_block(ui, cache, options, max_width);
            }

            pulldown_cmark::TagEnd::List(_) => {
                if self.list.is_last_level() {
                    self.line.should_start_newline = true;
                    self.line.should_end_newline = true;
                }

                self.list.end_level(ui, self.line.can_insert_end());

                if !self.list.is_inside_a_list() {
                    // Reset all the state and make it ready for the next list that occurs
                    self.list = List::default();
                }
            }
            pulldown_cmark::TagEnd::Item => {}
            pulldown_cmark::TagEnd::FootnoteDefinition => {
                self.line.should_start_newline = true;
                self.line.should_end_newline = true;
                self.line.try_insert_end(ui);
            }
            pulldown_cmark::TagEnd::Table => {}
            pulldown_cmark::TagEnd::TableHead => {}
            pulldown_cmark::TagEnd::TableRow => {}
            pulldown_cmark::TagEnd::TableCell => {
                // Ensure space between cells
                ui.label("  ");
            }
            pulldown_cmark::TagEnd::Emphasis => {
                self.text_style.emphasis = false;
            }
            pulldown_cmark::TagEnd::Strong => {
                self.text_style.strong = false;
            }
            pulldown_cmark::TagEnd::Strikethrough => {
                self.text_style.strikethrough = false;
            }
            pulldown_cmark::TagEnd::Link => {
                if let Some(link) = self.link.take() {
                    let (scrolled, match_ys) = link.end(
                        ui,
                        cache,
                        options,
                        &mut self.deferred_scroll_to_heading,
                        self.want_scroll_to_active_match,
                        self.content_origin_y,
                    );
                    if scrolled {
                        self.want_scroll_to_active_match = false;
                    }
                    self.search_match_ys_scratch.extend(match_ys);
                }
            }
            pulldown_cmark::TagEnd::Image => {
                if let Some(image) = self.image.take() {
                    let (height, scrolled, match_ys) = image.end(
                        ui,
                        cache,
                        options,
                        self.want_scroll_to_active_match,
                        self.content_origin_y,
                    );
                    if height < 1.0 {
                        self.any_image_loading = true;
                    }
                    if scrolled {
                        self.want_scroll_to_active_match = false;
                    }
                    self.search_match_ys_scratch.extend(match_ys);
                }
            }
            pulldown_cmark::TagEnd::HtmlBlock => {
                if let Some(html_fn) = options.html_fn {
                    html_fn(ui, &self.html_block);
                    self.html_block.clear();
                }
            }

            pulldown_cmark::TagEnd::MetadataBlock(_) => {}

            pulldown_cmark::TagEnd::DefinitionList => self.line.try_insert_end(ui),
            pulldown_cmark::TagEnd::DefinitionListTitle
            | pulldown_cmark::TagEnd::DefinitionListDefinition => {}
            pulldown_cmark::TagEnd::Superscript | pulldown_cmark::TagEnd::Subscript => {}
        }
    }

    fn end_code_block(
        &mut self,
        ui: &mut Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        if let Some(block) = self.code_block.take() {
            let (scrolled, match_ys) = block.end(
                ui,
                cache,
                options,
                max_width,
                self.want_scroll_to_active_match,
                self.content_origin_y,
            );
            #[cfg(feature = "regex")]
            if scrolled {
                self.want_scroll_to_active_match = false;
            }
            #[cfg(feature = "regex")]
            self.search_match_ys_scratch.extend(match_ys);
            #[cfg(not(feature = "regex"))]
            let _ = (scrolled, match_ys);
            if self.line.should_end_newline_forced {
                newline(ui);
            }
        }
    }
}

fn apply_pending_scroll_delta(vc: &mut ViewerCache, ui: &Ui) {
    let delta = std::mem::replace(&mut vc.pending_scroll_delta, egui::Vec2::ZERO);
    if delta != egui::Vec2::ZERO {
        ui.scroll_with_delta(delta);
    }
}

#[cfg(all(test, feature = "regex"))]
mod perf_tests {
    use egui::Id;

    use super::*;
    use crate::{CommonMarkCache, CommonMarkScrollOptions, CommonMarkViewer};

    /// Returns the number of full-document renders recorded for `id`
    /// since the process started (or since the entry was first created). Each
    /// `id` has its own counter, so parallel tests do not interfere.
    fn full_render_count_for(id: Id) -> usize {
        *FULL_RENDER_COUNTS.lock().unwrap().get(&id).unwrap_or(&0)
    }

    fn big_document() -> String {
        let mut text = String::new();
        for i in 1..=1024_usize {
            text += &format!(
                "\n## Section {i}\n\nThis is section {i}.\n\n```rs\nvec.push({i});\n```\n\n"
            );
        }
        text
    }

    fn windowed_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        }
    }

    /// Reproduces a bug where clicking "Next/Previous" was pathologically
    /// slow even when the target match was already on the currently visible
    /// page, because every call to `scroll_to_active_search_match` forced a
    /// full document re-render regardless of visibility.
    #[test]
    fn clicking_next_on_visible_match_is_fast() {
        let source_id_str = "perf_test_next_click";
        let source_id = Id::unique(source_id_str);
        let doc = big_document();
        // Three matches, all near the very top of the document (sections
        // 1-3), so they're all visible in the initial (unscrolled) viewport.
        let ranges: Vec<std::ops::Range<usize>> = [1, 2, 3]
            .iter()
            .map(|i| {
                let query = format!("This is section {i}.");
                let pos = doc.find(&query).expect("expected to find section text");
                pos..pos + query.len()
            })
            .collect();
        let mut cache = CommonMarkCache::default();
        let search_cache = cache.search_cache_mut(&source_id);
        search_cache.set_search_ranges(ranges.clone());
        search_cache.set_active_search_range(ranges.first().cloned());

        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());

        // Frame 0: cold render, populates page_size/split_points. Not timed.
        let output = ctx.run_ui(windowed_input(), |ui| {
            ui.set_min_height(600.0);
            CommonMarkViewer::new().show_scrollable(
                source_id,
                ui,
                &mut cache,
                &CommonMarkScrollOptions::default().viewport_cache(true),
                &doc,
            );
        });
        output.drop_without_applying_deltas();

        // Now simulate repeatedly clicking "Next", cycling among the 3
        // nearby matches. None of these require scrolling far, so each
        // should resolve within the fast viewport-only path.
        let mut worst: std::time::Duration = std::time::Duration::ZERO;
        for i in 0..12 {
            let active = &ranges[i % ranges.len()];
            let search_cache = cache.search_cache_mut(&source_id);
            search_cache.set_active_search_range(Some(active.clone()));
            search_cache.scroll_to_active_search_match();

            let click_start = std::time::Instant::now();
            let output = ctx.run_ui(windowed_input(), |ui| {
                ui.set_min_height(600.0);
                CommonMarkViewer::new().show_scrollable(
                    source_id,
                    ui,
                    &mut cache,
                    &CommonMarkScrollOptions::default().viewport_cache(true),
                    &doc,
                );
            });

            output.drop_without_applying_deltas();
            worst = worst.max(click_start.elapsed());
        }

        assert!(
            worst < std::time::Duration::from_millis(500),
            "clicking Next/Previous on an on-screen match should be fast, \
             worst frame took {worst:?}"
        );
    }

    /// Jumping to a match far outside the current viewport (e.g. near the
    /// end of a 1024-section document) must converge via cheap blind-scroll
    /// nudges over a few frames, never by forcing a full document re-render.
    #[test]
    fn jumping_to_offscreen_match_does_not_force_full_render() {
        let source_id_str = "offscreen_match";
        let source_id = Id::unique(source_id_str);
        let doc = big_document();
        let query = "This is section 1000.";
        let pos = doc.find(query).expect("expected to find section text");
        let target = pos..pos + query.len();

        let mut cache = CommonMarkCache::default();
        let search_cache = cache.search_cache_mut(&source_id);
        search_cache.set_search_ranges(vec![target.clone()]);

        // Real fonts, not `FontDefinitions::empty()`: with empty fonts, text
        // rows collapse to a few pixels tall, packing far more pulldown-cmark
        // events into any given viewport-height window than would ever
        // happen with real font metrics, which would make the per-frame
        // timing assertion below meaningless.
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());

        // The viewer uses its own cache entry (distinct from the search setup
        // above) so this test exercises viewport caching independently.
        let viewer_id = Id::unique("perf_test_offscreen_jump");

        // Frame 0: cold render at the top of the document, populates
        // page_size/split_points.
        let output = ctx.run_ui(windowed_input(), |ui| {
            ui.set_min_height(600.0);
            CommonMarkViewer::new().show_scrollable(
                viewer_id,
                ui,
                &mut cache,
                &CommonMarkScrollOptions::default().viewport_cache(true),
                &doc,
            );
        });
        output.drop_without_applying_deltas();

        let search_cache = cache.search_cache_mut(&source_id);
        search_cache.set_active_search_range(Some(target));
        search_cache.scroll_to_active_search_match();

        let before = full_render_count_for(viewer_id);
        let mut worst = std::time::Duration::ZERO;
        for _ in 0..10 {
            let frame_start = std::time::Instant::now();
            let output = ctx.run_ui(windowed_input(), |ui| {
                ui.set_min_height(600.0);
                CommonMarkViewer::new().show_scrollable(
                    viewer_id,
                    ui,
                    &mut cache,
                    &CommonMarkScrollOptions::default().viewport_cache(true),
                    &doc,
                );
            });

            output.drop_without_applying_deltas();
            worst = worst.max(frame_start.elapsed());
        }
        let delta = full_render_count_for(viewer_id) - before;

        assert_eq!(
            delta, 0,
            "jumping to an off-screen match must not force a full re-render"
        );
        assert!(
            worst < std::time::Duration::from_millis(250),
            "every frame while converging on an off-screen match should stay cheap, \
             worst frame took {worst:?}"
        );
    }

    #[test]
    fn steady_state_search_does_not_force_full_render_every_frame() {
        let source_id_str = "steady_state_search";
        let source_id = Id::unique(source_id_str);
        let doc = big_document();
        // A handful of matches, similar to a real search with few hits.
        let query = "push(123)";
        let mut ranges = Vec::new();
        let mut start = 0;
        while let Some(pos) = doc[start..].find(query) {
            let s = start + pos;
            ranges.push(s..s + query.len());
            start = s + query.len();
        }
        assert!(
            !ranges.is_empty(),
            "test setup: expected at least one match"
        );

        let mut cache = CommonMarkCache::default();
        let search_cache = cache.search_cache_mut(&source_id);
        search_cache.set_search_ranges(ranges.clone());
        search_cache.set_active_search_range(ranges.first().cloned());
        // Note: `scroll_to_active_search_match()` is deliberately NOT called here;
        // we're testing the steady state where matches exist but no scroll/jump
        // has been requested (e.g. the user has merely typed a query).

        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::empty());

        // Each test uses its own `source_id`, so `full_render_count_for()` is
        // keyed per source and cannot be polluted by other tests running in
        // parallel.
        let viewer_id = Id::unique("perf_test_doc");
        let before = full_render_count_for(viewer_id);
        const FRAMES: usize = 5;
        for _ in 0..FRAMES {
            let output = ctx.run_ui(windowed_input(), |ui| {
                ui.set_min_height(600.0);
                CommonMarkViewer::new().show_scrollable(
                    viewer_id,
                    ui,
                    &mut cache,
                    &CommonMarkScrollOptions::default().viewport_cache(true),
                    &doc,
                );
            });
            output.drop_without_applying_deltas();
        }

        let delta = full_render_count_for(viewer_id) - before;
        assert!(
            delta <= 1,
            "expected at most 1 full render across {FRAMES} steady-state frames, got {delta}"
        );
    }
}

/// Kladd: kolumnbredder för en tabell så att den får plats på `avail`. Korta kolumner får sin
/// naturliga bredd och de långa delar på resten (och radbryts).
/// Kladd: justerar ett stycke genom att lägga extra bokstavsavstånd vid blankstegen i varje rad utom den sista
/// (och rader som slutar med en hård radbrytning), så att raden fyller `wrap_w`. Radbrytningarna tas ur en första
/// utläggning utan justering; indraget (`leading_space` på första sektionen) bevaras. Texten ändras inte.
fn justify_job(ui: &Ui, job: LayoutJob, wrap_w: f32) -> LayoutJob {
    struct Plan {
        row: usize,
        a: usize,
        b: usize,
        /// Glyfindex för blankstegen mitt i raden och deras byteområde.
        spaces: Vec<(usize, Range<usize>)>,
        /// Önskad högerkant för sista ordet.
        end_x: f32,
        /// Extra bildpunkter per blanksteg hittills.
        units: Vec<usize>,
    }
    let layout = |j: &LayoutJob| ui.fonts_mut(|f| f.layout_job(j.clone()));
    let mut job = job;
    let first = layout(&job);
    if first.rows.len() < 2 {
        return job;
    }
    // egui radbryter först när ordet och blankstegen efter det ryms: tillåt ett blanksteg extra, så att sista ordet
    // på en justerad rad kan sluta exakt vid `wrap_w` (blankstegen sticker ut utanför kanten och syns inte).
    let space_w = first.rows.iter().flat_map(|r| r.row.glyphs.iter()).find(|g| g.chr == ' ').map_or(0.0, |g| g.logical_rect().width());
    job.wrap.max_width = wrap_w + space_w;
    let galley = layout(&job);
    let n_rows = galley.rows.len();
    if n_rows < 2 {
        return job;
    }
    let chars: Vec<(usize, char)> = job.text.char_indices().collect();
    let pixel = 1.0 / ui.pixels_per_point().max(0.5);
    let mut plans: Vec<Plan> = Vec::new();
    let mut ci = 0usize;
    for (ri, pr) in galley.rows.iter().enumerate() {
        let gl = &pr.row.glyphs;
        if ri + 1 < n_rows && !pr.ends_with_newline {
            let a = gl.iter().position(|g| !g.chr.is_whitespace());
            let b = gl.iter().rposition(|g| !g.chr.is_whitespace());
            if let (Some(a), Some(b)) = (a, b) {
                let in_sync = (a..=b).all(|i| chars.get(ci + i).map(|c| c.1) == Some(gl[i].chr));
                let spaces: Vec<(usize, Range<usize>)> = (a + 1..b)
                    .filter(|i| gl[*i].chr.is_whitespace())
                    .map(|i| {
                        // egui lägger bokstavsavståndet före varje tecken i sektionen, så blankstegets egen sektion
                        // vidgar mellanrummet före det.
                        let (byte, ch) = chars[ci + i];
                        (i, byte..byte + ch.len_utf8())
                    })
                    .collect();
                if in_sync && !spaces.is_empty() {
                    let n = spaces.len();
                    plans.push(Plan { row: ri, a, b, spaces, end_x: wrap_w - 1.5, units: vec![0; n] });
                }
            }
        }
        ci += pr.char_count_including_newline().0;
    }
    let apply = |plans: &[Plan]| -> LayoutJob {
        let mut gaps: Vec<(Range<usize>, f32)> = Vec::new();
        for p in plans {
            for (k, (_, r)) in p.spaces.iter().enumerate() {
                if p.units[k] > 0 {
                    gaps.push((r.clone(), p.units[k] as f32 * pixel));
                }
            }
        }
        let mut j = job.clone();
        egui_commonmark_backend::search::format_ranges(&mut j, &gaps, |f, e| f.extra_letter_spacing = *e);
        j
    };
    // Fördelar `units` hela bildpunkter jämnt över radens blanksteg.
    fn spread(p: &mut Plan, units: usize) {
        let k = p.spaces.len();
        let (base, rem) = (units / k, units % k);
        for n in 0..k {
            p.units[n] += base + ((n + 1) * rem / k - n * rem / k);
        }
    }
    // Några varv: utläggningen avrundar glyfernas lägen, så det som saknas mäts om och fylls på.
    let mut current = galley;
    for _ in 0..5 {
        let mut changed = false;
        for p in &mut plans {
            let gl = &current.rows[p.row].row.glyphs;
            let (Some(first), Some(last)) = (gl.get(p.a), gl.get(p.b)) else { continue };
            let natural_end = last.logical_rect().max.x;
            let _ = first;
            let units = ((p.end_x - natural_end) / pixel).floor();
            if units >= 1.0 {
                spread(p, units as usize);
                changed = true;
            }
        }
        if !changed {
            break;
        }
        let next = layout(&apply(&plans));
        // Andra radbrytningar än i första utläggningen: lämna justeringen som den var.
        if next.rows.len() != n_rows {
            break;
        }
        current = next;
    }
    apply(&plans)
}

/// Kladd: versaler med samma byte-längd som originalet (tecken som skulle ändra längd, som ß, lämnas).
fn upper_same_len(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) if u.len_utf8() == c.len_utf8() => u,
                _ => c,
            }
        })
        .collect()
}

/// Kladd: tecknet (index) under `ptr` i ett galleri som ritats vid `pos`, om pekaren är över en textrad.
fn galley_char_at(galley: &std::sync::Arc<egui::Galley>, pos: Pos2, ptr: Pos2) -> Option<usize> {
    let hit = galley.rows.iter().any(|r| r.rect().translate(pos.to_vec2()).contains(ptr));
    hit.then(|| galley.cursor_from_pos(ptr - pos).index.0)
}

fn table_column_widths(chars: &[usize], avail: f32, char_w: f32) -> Vec<f32> {
    const PAD: f32 = 18.0 + 8.0;
    const MIN: f32 = 72.0;
    let est: Vec<f32> = chars.iter().map(|&n| n as f32 * char_w + PAD).collect();
    let total: f32 = est.iter().sum();
    if total <= avail {
        // Plats för allt: lite luft så att fetstil och bredare tecken inte radbryts i onödan.
        return est.iter().map(|e| e * 1.2 + 6.0).collect();
    }
    let fair = avail / est.len().max(1) as f32;
    let mut out = est.clone();
    let mut remaining = avail;
    let mut flexible: Vec<usize> = Vec::new();
    for (i, &e) in est.iter().enumerate() {
        if e <= fair {
            remaining -= e;
        } else {
            flexible.push(i);
        }
    }
    let flex_total: f32 = flexible.iter().map(|&i| est[i]).sum();
    for &i in &flexible {
        out[i] = (remaining * est[i] / flex_total.max(1.0)).max(MIN);
    }
    out
}
