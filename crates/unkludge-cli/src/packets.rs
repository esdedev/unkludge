//! Showing what goes to the keyboard: the reports of a dry run and the latency of acks.

use std::time::Duration;

use unkludge_protocol::device::Collection;

/// The command byte of screen data chunks.
pub const SCREEN_DATA: u8 = 0x0C;
/// The command byte of screen control reports.
pub const SCREEN_CONTROL: u8 = 0x0D;

const HEADER_LEN: usize = 8;
/// How much of a screen chunk gets shown, header included, unless it's asked for in full.
const CHUNK_PREVIEW: usize = 24;

/// A report as hex, with the zeros that fill the end of it collapsed. The header always shows.
/// `limit` cuts it short after that many bytes.
pub fn format_report(report: &[u8], limit: Option<usize>) -> String {
    let used = report
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |last| last + 1)
        .max(HEADER_LEN.min(report.len()));
    let shown = limit.map_or(used, |limit| used.min(limit));
    let mut text = hex(&report[..shown]);
    if shown < used {
        text.push_str(&format!(" (+{} bytes)", report.len() - shown));
    } else if used < report.len() {
        text.push_str(&format!(" (+{} zeros)", report.len() - used));
    }
    text
}

/// Bytes as upper-case hex, separated by spaces.
pub fn hex(bytes: &[u8]) -> String {
    let pairs: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    pairs.join(" ")
}

/// Turns the reports of a dry run into lines of text. An upload has thousands of chunks, so each
/// frame shows its first and last, and a count of those between them, unless `verbose` asks for
/// everything.
pub struct PacketPrinter {
    verbose: bool,
    two_collections: bool,
    frame: Option<FrameRun>,
    /// Reports per command byte, in the order they first appeared.
    counts: Vec<(u8, usize)>,
}

/// The chunks of one frame seen so far. The first has already been printed.
struct FrameRun {
    number: u8,
    chunks: usize,
    last: Vec<u8>,
    collection: Collection,
}

impl PacketPrinter {
    /// `two_collections` says whether to show which collection each report goes to.
    pub fn new(verbose: bool, two_collections: bool) -> Self {
        PacketPrinter {
            verbose,
            two_collections,
            frame: None,
            counts: Vec::new(),
        }
    }

    /// The lines to print for this report, which may be none.
    pub fn on_sent(&mut self, collection: Collection, report: &[u8]) -> Vec<String> {
        let command = report.get(1).copied().unwrap_or(0);
        match self.counts.iter_mut().find(|(c, _)| *c == command) {
            Some((_, count)) => *count += 1,
            None => self.counts.push((command, 1)),
        }

        if self.verbose {
            return vec![self.line(collection, report, None)];
        }
        if command != SCREEN_DATA {
            let mut lines = self.end_frame();
            lines.push(self.line(collection, report, None));
            return lines;
        }

        let number = report.get(3).copied().unwrap_or(0);
        if let Some(run) = self.frame.as_mut().filter(|run| run.number == number) {
            run.chunks += 1;
            run.last.clear();
            run.last.extend_from_slice(report);
            run.collection = collection;
            return Vec::new();
        }
        let mut lines = self.end_frame();
        lines.push(self.line(collection, report, Some(CHUNK_PREVIEW)));
        self.frame = Some(FrameRun {
            number,
            chunks: 1,
            last: report.to_vec(),
            collection,
        });
        lines
    }

    /// The lines still to print, and how many reports of each command there were.
    pub fn finish(&mut self) -> Vec<String> {
        let mut lines = self.end_frame();
        let counts: Vec<String> = self
            .counts
            .iter()
            .map(|(command, count)| format!("{count} x {command:#04X}"))
            .collect();
        lines.push(format!(
            "Reports: {}",
            if counts.is_empty() {
                "none".to_string()
            } else {
                counts.join(", ")
            }
        ));
        lines
    }

    fn end_frame(&mut self) -> Vec<String> {
        let Some(run) = self.frame.take() else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        if run.chunks > 2 {
            lines.push(format!(
                "... {} more chunks of frame {} ...",
                run.chunks - 2,
                run.number
            ));
        }
        if run.chunks > 1 {
            lines.push(self.line(run.collection, &run.last, Some(CHUNK_PREVIEW)));
        }
        lines
    }

    fn line(&self, collection: Collection, report: &[u8], limit: Option<usize>) -> String {
        let text = format_report(report, limit);
        match (self.two_collections, collection) {
            (false, _) => text,
            (true, Collection::Main) => format!("main:   {text}"),
            (true, Collection::Screen) => format!("screen: {text}"),
        }
    }
}

/// The latency of the acks seen.
#[derive(Default)]
pub struct Timing {
    latencies: Vec<Duration>,
    refused: usize,
}

impl Timing {
    pub fn on_ack(&mut self, accepted: bool, latency: Duration) {
        self.latencies.push(latency);
        self.refused += usize::from(!accepted);
    }

    pub fn has_acks(&self) -> bool {
        !self.latencies.is_empty()
    }

    /// One line on all the acks: how many, then the minimum, median and maximum latency.
    pub fn summary(&self) -> String {
        let mut sorted = self.latencies.clone();
        sorted.sort_unstable();
        let (Some(&min), Some(&max)) = (sorted.first(), sorted.last()) else {
            return "Acks: none".to_string();
        };
        let middle = sorted.len() / 2;
        let median = if sorted.len().is_multiple_of(2) {
            (sorted[middle - 1] + sorted[middle]) / 2
        } else {
            sorted[middle]
        };
        let refused = if self.refused > 0 {
            format!(" ({} refused)", self.refused)
        } else {
            String::new()
        };
        format!(
            "Acks: {}{refused}, min {}, median {}, max {}",
            sorted.len(),
            millis(min),
            millis(median),
            millis(max)
        )
    }
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::REPORT_LEN;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    fn report(bytes: &[u8]) -> Vec<u8> {
        let mut report = vec![0; REPORT_LEN];
        report[..bytes.len()].copy_from_slice(bytes);
        report
    }

    fn chunk(frame: u8, idx: u8) -> Vec<u8> {
        let mut chunk = report(&[0x09, SCREEN_DATA, 0, frame, idx, 0, 0x00, 0x02]);
        chunk[8..].fill(0xF8);
        chunk
    }

    fn print_all(printer: &mut PacketPrinter, reports: &[Vec<u8>]) -> Vec<String> {
        let mut lines: Vec<String> = reports
            .iter()
            .flat_map(|report| printer.on_sent(Collection::Main, report))
            .collect();
        lines.extend(printer.finish());
        lines
    }

    #[test]
    fn trailing_zeros_collapse() {
        assert_eq!(
            format_report(
                &report(&[0x09, 0x0B, 0, 0, 1, 0, 0x0E, 0, 0, 0, 0, 0xEA]),
                None
            ),
            "09 0B 00 00 01 00 0E 00 00 00 00 EA (+508 zeros)"
        );
    }

    #[test]
    fn the_header_shows_even_when_it_ends_in_zeros() {
        assert_eq!(
            format_report(&report(&[0x09, 0x84, 0, 0, 1, 0, 0x80, 0]), None),
            "09 84 00 00 01 00 80 00 (+512 zeros)"
        );
        assert_eq!(
            format_report(&report(&[0x09]), None),
            "09 00 00 00 00 00 00 00 (+512 zeros)"
        );
    }

    #[test]
    fn a_full_report_has_nothing_to_collapse() {
        let full = vec![0xAB; REPORT_LEN];
        let text = format_report(&full, None);
        assert_eq!(text.len(), REPORT_LEN * 3 - 1);
        assert!(!text.contains('+'));
    }

    #[test]
    fn a_limit_cuts_the_report_short() {
        let full = vec![0xAB; REPORT_LEN];
        assert_eq!(format_report(&full, Some(2)), "AB AB (+518 bytes)");
    }

    #[test]
    fn a_limit_past_the_data_only_collapses_zeros() {
        assert_eq!(
            format_report(&report(&[0x09, 0x0C, 0, 0, 0, 0, 2, 0, 0xF8]), Some(24)),
            "09 0C 00 00 00 00 02 00 F8 (+511 zeros)"
        );
    }

    #[test]
    fn other_reports_print_in_full_one_line_each() {
        let mut printer = PacketPrinter::new(false, false);
        let mut config = report(&[0x09, 0x04, 0, 0, 1, 0, 0x80, 0]);
        config[8..136].fill(0x11);
        let lines = print_all(&mut printer, std::slice::from_ref(&config));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("11 11 (+384 zeros)"), "{}", lines[0]);
        assert_eq!(lines[1], "Reports: 1 x 0x04");
    }

    #[test]
    fn a_frame_shows_its_first_and_last_chunk_and_counts_the_rest() {
        let mut printer = PacketPrinter::new(false, false);
        let mut reports = vec![report(&[0x09, SCREEN_CONTROL, 0, 0, 1, 0, 5, 0, 0x40])];
        reports.extend((0..215).map(|idx| chunk(0, idx as u8)));
        reports.extend((0..215).map(|idx| chunk(1, idx as u8)));
        reports.push(report(&[0x09, SCREEN_CONTROL, 0, 0, 1, 0, 5, 0, 0x00, 2]));
        let lines = print_all(&mut printer, &reports);

        let preview = "F8 F8 F8 F8 F8 F8 F8 F8 F8 F8 F8 F8 F8 F8 F8 F8 (+496 bytes)";
        let expected = [
            "09 0D 00 00 01 00 05 00 40 (+511 zeros)".to_string(),
            format!("09 0C 00 00 00 00 00 02 {preview}"),
            "... 213 more chunks of frame 0 ...".to_string(),
            format!("09 0C 00 00 D6 00 00 02 {preview}"),
            format!("09 0C 00 01 00 00 00 02 {preview}"),
            "... 213 more chunks of frame 1 ...".to_string(),
            format!("09 0C 00 01 D6 00 00 02 {preview}"),
            "09 0D 00 00 01 00 05 00 00 02 (+510 zeros)".to_string(),
            "Reports: 2 x 0x0D, 430 x 0x0C".to_string(),
        ];
        assert_eq!(lines, expected);
    }

    #[test]
    fn short_frames_have_no_count() {
        let mut printer = PacketPrinter::new(false, false);
        let lines = print_all(&mut printer, &[chunk(0, 0)]);
        assert_eq!(lines.len(), 2, "{lines:?}");

        let mut printer = PacketPrinter::new(false, false);
        let lines = print_all(&mut printer, &[chunk(0, 0), chunk(0, 1)]);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines.iter().all(|line| !line.contains("more chunks")));
    }

    #[test]
    fn verbose_prints_every_chunk_in_full() {
        let mut printer = PacketPrinter::new(true, false);
        let lines = print_all(&mut printer, &[chunk(0, 0), chunk(0, 1), chunk(0, 2)]);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1].len(), REPORT_LEN * 3 - 1);
        assert_eq!(lines[3], "Reports: 3 x 0x0C");
    }

    #[test]
    fn two_collections_are_told_apart() {
        let mut printer = PacketPrinter::new(false, true);
        let control = report(&[0x09, SCREEN_CONTROL, 0, 0, 1, 0, 5, 0, 0x40]);
        let main = printer.on_sent(Collection::Main, &control);
        let screen = printer.on_sent(Collection::Screen, &control);
        assert!(main[0].starts_with("main:   09 0D"), "{main:?}");
        assert!(screen[0].starts_with("screen: 09 0D"), "{screen:?}");
    }

    #[test]
    fn no_reports_say_so() {
        assert_eq!(PacketPrinter::new(false, false).finish(), ["Reports: none"]);
    }

    #[test]
    fn no_acks_say_so() {
        assert_eq!(Timing::default().summary(), "Acks: none");
    }

    #[test]
    fn timing_gives_min_median_and_max() {
        let mut timing = Timing::default();
        for ms in [30, 10, 20] {
            timing.on_ack(true, MS(ms));
        }
        assert_eq!(
            timing.summary(),
            "Acks: 3, min 10.0 ms, median 20.0 ms, max 30.0 ms"
        );
    }

    #[test]
    fn an_even_count_takes_the_mean_of_the_middle_two() {
        let mut timing = Timing::default();
        for ms in [40, 10, 30, 15] {
            timing.on_ack(true, MS(ms));
        }
        assert_eq!(
            timing.summary(),
            "Acks: 4, min 10.0 ms, median 22.5 ms, max 40.0 ms"
        );
    }

    #[test]
    fn refused_acks_are_counted() {
        let mut timing = Timing::default();
        timing.on_ack(false, MS(5));
        timing.on_ack(true, MS(5));
        assert_eq!(
            timing.summary(),
            "Acks: 2 (1 refused), min 5.0 ms, median 5.0 ms, max 5.0 ms"
        );
    }
}
