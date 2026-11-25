//! Responsive placement for the native recorder controls.
//!
//! Coordinates are calculated in 96-DPI logical units, then scaled once for
//! the current display. The window owns the vertical scroll bar.

use super::native_window::*;

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

struct Rows<'a, F: Fn(i32) -> bool> {
    width: i32,
    x: i32,
    y: i32,
    row_height: i32,
    visible: &'a F,
    rects: Vec<(i32, Rect)>,
}

impl<'a, F: Fn(i32) -> bool> Rows<'a, F> {
    fn new(width: i32, visible: &'a F) -> Self {
        Self {
            width: width.max(32),
            x: 10,
            y: 10,
            row_height: 0,
            visible,
            rects: Vec::new(),
        }
    }

    fn add(&mut self, id: i32, preferred: i32, height: i32) {
        if !(self.visible)(id) {
            return;
        }
        let available = (self.width - 20).max(1);
        let item_width = preferred.min(available);
        if self.x > 10 && self.x + item_width > self.width - 10 {
            self.next();
        }
        self.rects.push((
            id,
            Rect {
                x: self.x,
                y: self.y,
                width: item_width,
                height,
            },
        ));
        self.x += item_width + 8;
        self.row_height = self.row_height.max(height);
    }

    fn fill(&mut self, id: i32, height: i32) {
        self.add(id, self.width - 20, height);
    }

    fn heading(&mut self, id: i32) {
        self.fill(id, 21);
        self.next();
    }

    fn field(
        &mut self,
        label: i32,
        label_width: i32,
        id: i32,
        preferred: i32,
        minimum: i32,
        height: i32,
    ) {
        if !(self.visible)(id) {
            self.add(label, label_width, 22);
            return;
        }
        if !(self.visible)(label) {
            self.add(id, preferred, height);
            return;
        }

        let needed = label_width + 8 + minimum;
        if self.x + needed > self.width - 10 {
            self.next();
        }
        self.add(label, label_width, 22);
        if needed > self.width - 20 {
            self.next();
        }
        self.add(id, preferred.min((self.width - self.x - 10).max(1)), height);
    }

    fn path(&mut self, label: i32, label_width: i32, id: i32, browse: i32) {
        const BROWSE_WIDTH: i32 = 81;
        const MINIMUM_PATH: i32 = 112;
        let available = self.width - 20 - label_width - 16 - BROWSE_WIDTH;
        let preferred = if available < MINIMUM_PATH {
            self.width
        } else {
            available
        };
        self.field(label, label_width, id, preferred, MINIMUM_PATH, 27);
        self.add(browse, BROWSE_WIDTH, 27);
    }

    fn readout(&mut self, label: i32, label_width: i32, id: i32, height: i32) {
        if self.width < 440 {
            self.heading(label);
            self.fill(id, height);
        } else {
            self.field(label, label_width, id, self.width, 139, height);
        }
        self.next();
    }

    fn next(&mut self) {
        if self.row_height > 0 {
            self.y += self.row_height + 7;
        }
        self.x = 10;
        self.row_height = 0;
    }
}

fn positions<F: Fn(i32) -> bool>(width: i32, visible: &F) -> (Vec<(i32, Rect)>, i32) {
    let mut row = Rows::new(width, visible);
    let input = 27;
    let button = 30;

    row.heading(SOURCES_SECTION);
    row.add(SELECT_PROGRAMS, 143, button);
    row.add(RECENT_OPTIONS, 156, button);
    row.next();
    row.field(RECENT_LABEL, 105, RECENT_PICKER, 150, 140, input);
    row.add(USE_RECENT, 94, input);
    row.next();
    row.add(AUTO_SELECT_RECENT, 144, input);
    row.add(FORGET_RECENT, 99, input);
    row.add(CLEAR_RECENT, 99, input);
    row.next();
    row.field(MIC_LABEL, 105, MIC, 290, 160, input);
    row.next();
    row.fill(SOURCE_SUMMARY, 21);
    row.next();
    row.fill(PROCESS_LABEL, if width < 300 { 35 } else { 22 });
    row.next();
    row.fill(PROCESS_LIST, 125);
    row.next();
    row.add(REFRESH, 86, input);
    row.add(FILTER, 237, if width < 260 { 40 } else { input });
    row.add(SYSTEM, 174, input);
    row.next();

    row.heading(RECORDING_SECTION);
    row.add(START, 116, button);
    row.add(PAUSE, 124, button);
    row.add(MARK, 97, button);
    row.add(RECORDING_SETTINGS, 159, button);
    row.next();
    row.readout(RECORDING_STATUS_LABEL, 107, STATUS, 58);

    row.heading(LIVE_SECTION);
    row.add(LIVE_TOGGLE, 167, button);
    row.add(STOP_BOTH, 85, button);
    row.add(TRANSCRIPTION_SETTINGS, 173, button);
    row.next();
    row.readout(LIVE_STATUS_LABEL, 122, LIVE_STATUS, 58);
    row.add(LIVE_OPEN_TEXT, 111, input);
    row.add(LIVE_OPEN_FOLDER, 94, input);
    row.add(LIVE_CANCEL_PENDING, 156, input);
    row.next();
    row.readout(LIVE_PREVIEW_LABEL, 107, LIVE_PREVIEW, 106);

    row.heading(BATCH_SECTION);
    row.add(TRANSCRIBE_FILES, 145, button);
    row.add(BATCH_MANAGE, 121, button);
    row.next();

    row.heading(RECORDING_SETTINGS_SECTION);
    row.field(PROFILE_LABEL, 114, PROFILE, PROFILE_WIDTH, 250, input);
    row.next();
    row.fill(PROFILE_HELP, if width < 475 { 40 } else { 23 });
    row.next();
    row.field(FORMAT_LABEL, 75, FORMAT, 214, 160, input);
    row.field(CHANNEL_LABEL, 75, CHANNEL, 105, 90, input);
    row.next();
    row.field(BITRATE_LABEL, 154, BITRATE, 123, 88, input);
    row.field(SAMPLE_LABEL, 112, SAMPLE, 140, 105, input);
    row.next();

    row.field(PRESET_LABEL, 105, PRESET, 205, 111, input);
    row.add(SAVE, 94, input);
    row.add(LOAD, 94, input);
    row.add(DELETE, 100, input);
    row.next();
    row.field(MONITOR_LABEL, 110, MONITOR, 215, 137, input);
    row.add(MONITOR_ONLY, 128, input);
    row.next();
    row.field(APP_GAIN_LABEL, 125, APP_GAIN, 150, 103, input);
    row.field(MIC_GAIN_LABEL, 125, MIC_GAIN, 150, 103, input);
    row.next();
    row.field(MODE_LABEL, 110, MODE, 171, 128, input);
    row.add(SILENCE, 113, input);
    row.next();
    row.path(FOLDER_LABEL, 110, FOLDER, BROWSE);
    row.next();

    row.heading(TRANSCRIPTION_SETTINGS_SECTION);
    row.add(LOAD_LIVE_CONFIG, 125, input);
    row.add(SAVE_LIVE_CONFIG, 125, input);
    row.next();
    row.field(LIVE_ENDPOINT_LABEL, 177, LIVE_ENDPOINT, width, 145, input);
    row.next();
    row.field(LIVE_MODEL_LABEL, 177, LIVE_MODEL, width, 145, input);
    row.next();
    row.field(LIVE_KEY_ENV_LABEL, 177, LIVE_KEY_ENV, width, 145, input);
    row.next();
    row.path(LIVE_OUTPUT_DIR_LABEL, 160, LIVE_OUTPUT_DIR, LIVE_BROWSE);
    row.next();
    row.add(LIVE_GATE, 205, input);
    row.add(LIVE_TIMESTAMPS, 193, input);
    row.next();
    row.add(LIVE_SPEAKERS, 205, input);
    row.next();
    row.field(
        LIVE_SPEAKER_CONFIG_LABEL,
        177,
        LIVE_SPEAKER_CONFIG,
        width,
        145,
        input,
    );
    row.next();

    row.fill(LEVELS_LABEL, 22);
    row.next();
    row.fill(LEVELS, 85);
    row.next();
    (row.rects, row.y + 3)
}

/// Place visible controls and return the full physical content height.
pub fn layout(view: &View, width: i32, _height: i32, dpi: u32, scroll: i32) -> i32 {
    let dpi = dpi.max(1) as i64;
    let logical_width = ((width.max(1) as i64 * 96) / dpi).max(1) as i32;
    let (rects, content_height) = positions(logical_width, &|id| view.visible(id));
    let scale = |value: i32| ((value as i64 * dpi + 48) / 96) as i32;
    for (id, rect) in rects {
        view.place(
            id,
            scale(rect.x),
            scale(rect.y) - scroll,
            scale(rect.width),
            scale(rect.height),
        );
    }
    scale(content_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_reported_positions_overlap() {
        let filter = Rect {
            x: 120,
            y: 248,
            width: 280,
            height: 20,
        };
        let bitrate = Rect {
            x: 120,
            y: 248,
            width: 80,
            height: 20,
        };
        assert!(overlaps(filter, bitrate));
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    fn rect(rects: &[(i32, Rect)], id: i32) -> Rect {
        rects.iter().find(|(control, _)| *control == id).unwrap().1
    }

    #[test]
    fn source_selection_and_both_activities_precede_advanced_settings() {
        for width in [280, 390, 660, 1024] {
            let (rects, _) = positions(width, &|_| true);
            let y = |id| rect(&rects, id).y;
            assert!(y(SELECT_PROGRAMS) < y(START));
            assert!(y(MIC) < y(START));
            assert!(y(START) < y(LIVE_TOGGLE));
            assert!(y(LIVE_TOGGLE) < y(TRANSCRIBE_FILES));
            assert!(y(TRANSCRIBE_FILES) < y(PROFILE));
            assert!(y(PROFILE) < y(FORMAT));
            assert!(y(FORMAT) < y(LIVE_ENDPOINT));
        }
    }

    #[test]
    fn compact_layout_places_primary_actions_within_first_page() {
        let hidden = [
            AUTO_SELECT_RECENT,
            FORGET_RECENT,
            CLEAR_RECENT,
            PROCESS_LABEL,
            PROCESS_LIST,
            REFRESH,
            FILTER,
            SYSTEM,
            PAUSE,
            MARK,
            STOP_BOTH,
            LIVE_OPEN_TEXT,
            LIVE_OPEN_FOLDER,
            LIVE_CANCEL_PENDING,
            LIVE_PREVIEW_LABEL,
            LIVE_PREVIEW,
        ];
        let settings = positions(390, &|_| true).0;
        let first_advanced = settings
            .iter()
            .position(|(id, _)| *id == RECORDING_SETTINGS_SECTION)
            .unwrap();
        let visible = |id| {
            !hidden.contains(&id)
                && settings[..first_advanced]
                    .iter()
                    .any(|(control, _)| *control == id)
        };
        for width in [390, 660] {
            let (rects, _) = positions(width, &visible);
            for id in [SELECT_PROGRAMS, START, LIVE_TOGGLE, TRANSCRIBE_FILES] {
                let control = rect(&rects, id);
                assert!(control.y + control.height <= 560);
            }
            assert!(rects.iter().all(|(id, _)| !hidden.contains(id)));
        }
    }

    #[test]
    fn narrow_status_and_preview_use_the_full_content_width() {
        let (rects, _) = positions(390, &|_| true);
        for (label, readout) in [
            (RECORDING_STATUS_LABEL, STATUS),
            (LIVE_STATUS_LABEL, LIVE_STATUS),
            (LIVE_PREVIEW_LABEL, LIVE_PREVIEW),
        ] {
            let label = rect(&rects, label);
            let readout = rect(&rects, readout);
            assert_eq!(readout.width, 370);
            assert!(readout.y >= label.y + label.height);
        }
    }

    #[test]
    fn dpi_matrix_keeps_primary_actions_and_codec_fields_separate() {
        for dpi in [96, 120, 144, 192] {
            for logical_width in [240, 280, 390, 660, 1024] {
                let (rects, content) = positions(logical_width, &|_| true);
                let scale = |n| (n * dpi + 48) / 96;
                let pixels = |r: Rect| Rect {
                    x: scale(r.x),
                    y: scale(r.y),
                    width: scale(r.width),
                    height: scale(r.height),
                };
                for (index, (id, r)) in rects.iter().enumerate() {
                    let bounds = pixels(*r);
                    assert!(
                        bounds.x >= 0 && bounds.x + bounds.width <= scale(logical_width),
                        "dpi {dpi}, width {logical_width}, id {id}, bounds {bounds:?}"
                    );
                    assert!(bounds.y >= 0 && bounds.y + bounds.height <= scale(content));
                    for (other_id, other) in &rects[index + 1..] {
                        assert!(
                            !overlaps(bounds, pixels(*other)),
                            "dpi {dpi}, width {logical_width}, id {id}, id {other_id}"
                        );
                    }
                }
                for (label, field, min_width) in
                    [(BITRATE_LABEL, BITRATE, 88), (SAMPLE_LABEL, SAMPLE, 105)]
                {
                    let label = rect(&rects, label);
                    let field = rect(&rects, field);
                    assert!(field.width >= min_width);
                    assert!(field.y == label.y || field.x == label.x);
                }
            }
        }
    }
}
