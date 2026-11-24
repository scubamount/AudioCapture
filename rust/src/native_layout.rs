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

    fn fill(&mut self, id: i32, reserved: i32, minimum: i32, height: i32) {
        let available = (self.width - 20).max(1);
        let preferred = (available - reserved).max(minimum).min(available);
        self.add(id, preferred, height);
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

    row.add(PRESET_LABEL, 52, 25);
    row.add(PRESET, 185, 25);
    row.add(SAVE, 94, 25);
    row.add(LOAD, 94, 25);
    row.add(DELETE, 100, 25);
    row.next();

    row.add(PROCESS_LABEL, 320, if width < 340 { 44 } else { 23 });
    row.next();
    row.fill(PROCESS_LIST, 0, 100, 132);
    row.next();
    row.add(REFRESH, 105, 26);
    let checkbox_height = if width < 320 { 44 } else { 26 };
    row.add(FILTER, 260, checkbox_height);
    row.add(SYSTEM, 180, checkbox_height);
    row.next();

    row.add(FORMAT_LABEL, 92, 26);
    row.add(FORMAT, 140, 26);
    row.next();
    row.add(BITRATE_LABEL, 165, 26);
    row.add(BITRATE, 130, 26);
    row.next();
    row.add(SAMPLE_LABEL, 165, 26);
    row.add(SAMPLE, 145, 26);
    row.next();
    row.add(CHANNEL_LABEL, 92, 26);
    row.add(CHANNEL, 205, 26);
    row.next();

    row.add(MIC_LABEL, 110, 26);
    row.add(MIC, 230, 26);
    row.next();
    row.add(MONITOR_LABEL, 110, 26);
    row.add(MONITOR, 230, 26);
    row.add(MONITOR_ONLY, 170, checkbox_height);
    row.next();
    row.add(APP_GAIN_LABEL, 120, 26);
    row.add(APP_GAIN, 230, 26);
    row.next();
    row.add(MIC_GAIN_LABEL, 120, 26);
    row.add(MIC_GAIN, 230, 26);
    row.next();

    row.add(MODE_LABEL, 110, 26);
    row.add(MODE, 180, 26);
    row.add(SILENCE, 170, checkbox_height);
    row.next();
    row.add(FOLDER_LABEL, 100, 26);
    row.fill(FOLDER, 205, 170, 26);
    row.add(BROWSE, 90, 26);
    row.next();
    row.add(START, 110, 30);
    row.add(PAUSE, 110, 30);
    row.add(STOP, 110, 30);
    row.next();
    row.fill(STATUS, 0, 100, 60);
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

    #[test]
    fn layout_keeps_all_visible_controls_separate_and_reachable() {
        for dpi in [96, 120, 144, 192] {
            for width in [1024, 768, 480] {
                for all_controls in [false, true] {
                    let visible = |id| {
                        all_controls || ![MONITOR_ONLY, MIC_GAIN, MIC_GAIN_LABEL].contains(&id)
                    };
                    let logical_width = width * 96 / dpi;
                    let (rects, height) = positions(logical_width, &visible);
                    for (index, (id, rect)) in rects.iter().enumerate() {
                        assert!(
                            rect.x >= 0 && rect.x + rect.width <= logical_width,
                            "dpi {dpi}, width {width}, id {id}, rect {rect:?}"
                        );
                        assert!(rect.y >= 0 && rect.y + rect.height <= height);
                        for (other_id, other) in &rects[index + 1..] {
                            assert!(
                                !overlaps(*rect, *other),
                                "dpi {dpi}, width {width}, id {id}, id {other_id}"
                            );
                        }
                    }
                    let filter = rects.iter().find(|(id, _)| *id == FILTER).unwrap().1;
                    let bitrate = rects.iter().find(|(id, _)| *id == BITRATE).unwrap().1;
                    let stop = rects.iter().find(|(id, _)| *id == STOP).unwrap().1;
                    assert!(!overlaps(filter, bitrate));
                    // With vertical scrolling any content coordinate can be brought
                    // inside a nonzero-height viewport.
                    assert!(height >= stop.y + stop.height);
                }
            }
        }
    }
}
