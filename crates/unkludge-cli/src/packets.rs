//! Showing what goes to the keyboard: the reports of a dry run and the latency of acks.

use std::fmt::{self, Display, Formatter};
use std::time::{Duration, Instant};

use unkludge_protocol::device::{Collection, PacketEvent};

use crate::output::Console;
use crate::report_lines::hex;

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

/// What it takes to get a chunk across, to find out why uploads are slow.
///
/// `Sent` is reported once a report has been handed over, and an ack's wait starts there, so the two
/// together cover all of a chunk's time: "send" runs from the previous chunk's ack to this chunk
/// being handed over, and "ack wait" from there to its ack. The protocol's pauses around the start
/// and the finish are left out of both, as they aren't between chunks.
#[derive(Default)]
pub struct Timing {
    first_sent: Option<Instant>,
    last_event: Option<Instant>,
    first_chunk: Option<Instant>,
    last_chunk_ack: Option<Instant>,
    waiting_for_chunk_ack: bool,
    this_send: Option<Duration>,
    chunks_sent: usize,
    sends: Vec<Duration>,
    chunk_waits: Vec<Duration>,
    control_waits: Vec<Duration>,
    /// From one chunk's ack to the next one's.
    cycles: Vec<Duration>,
    refused: usize,
}

impl Timing {
    pub fn on_sent(&mut self, command: u8, at: Instant) {
        self.first_sent.get_or_insert(at);
        self.last_event = Some(at);
        self.waiting_for_chunk_ack = command == SCREEN_DATA;
        self.this_send = None;
        if command == SCREEN_DATA {
            self.chunks_sent += 1;
            self.first_chunk.get_or_insert(at);
            if let Some(previous) = self.last_chunk_ack {
                let send = at.saturating_duration_since(previous);
                self.sends.push(send);
                self.this_send = Some(send);
            }
        }
    }

    /// Returns a line on this ack.
    pub fn on_ack(&mut self, idx: u8, accepted: bool, wait: Duration, at: Instant) -> String {
        self.last_event = Some(at);
        self.refused += usize::from(!accepted);
        if self.waiting_for_chunk_ack {
            self.chunk_waits.push(wait);
            if let Some(previous) = self.last_chunk_ack {
                self.cycles.push(at.saturating_duration_since(previous));
            }
            self.last_chunk_ack = Some(at);
        } else {
            self.control_waits.push(wait);
        }
        let send = self
            .this_send
            .map_or(String::new(), |send| format!(", send {}", millis(send)));
        let refused = if accepted { "" } else { ", refused" };
        format!("Ack {idx}: wait {}{send}{refused}", millis(wait))
    }

    /// What it all came to.
    pub fn summary(&self) -> Vec<String> {
        if self.chunk_waits.is_empty() && self.control_waits.is_empty() {
            return vec!["Acks: none".to_string()];
        }
        let mut lines = Vec::new();
        if let (Some(first), Some(last)) = (self.first_sent, self.last_event) {
            lines.push(format!(
                "Total: {:.1} s, from the first report to the last ack",
                last.saturating_duration_since(first).as_secs_f64()
            ));
        }
        if let (Some(first), Some(last)) = (self.first_chunk, self.last_chunk_ack) {
            let elapsed = last.saturating_duration_since(first);
            lines.push(format!(
                "Chunks: {} sent, {} acknowledged in {:.1} s, {} per second",
                self.chunks_sent,
                self.chunk_waits.len(),
                elapsed.as_secs_f64(),
                per_second(self.chunk_waits.len(), elapsed)
            ));
        }
        if let Some(stats) = Stats::of(&self.cycles) {
            lines.push(format!(
                "Chunk rate: min {}, median {}, max {} per second",
                per_second(1, stats.max),
                per_second(1, stats.median),
                per_second(1, stats.min)
            ));
        }
        if let Some(stats) = Stats::of(&self.sends) {
            lines.push(format!("Send: {stats}"));
        }
        if let Some(stats) = Stats::of(&self.chunk_waits) {
            lines.push(format!("Ack wait: {stats}"));
        }
        if let Some(stats) = Stats::of(&self.control_waits) {
            lines.push(format!("Start and finish ack wait: {stats}"));
        }
        if self.refused > 0 {
            lines.push(format!("Refused: {}", self.refused));
        }
        lines
    }
}

/// The spread of some durations.
struct Stats {
    min: Duration,
    median: Duration,
    max: Duration,
}

impl Stats {
    fn of(durations: &[Duration]) -> Option<Stats> {
        let mut sorted = durations.to_vec();
        sorted.sort_unstable();
        let middle = sorted.len() / 2;
        Some(Stats {
            min: *sorted.first()?,
            median: if sorted.len().is_multiple_of(2) {
                (sorted[middle - 1] + sorted[middle]) / 2
            } else {
                sorted[middle]
            },
            max: *sorted.last()?,
        })
    }
}

impl Display for Stats {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "min {}, median {}, max {}",
            millis(self.min),
            millis(self.median),
            millis(self.max)
        )
    }
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// `-` when no time has passed, as in a dry run.
fn per_second(count: usize, duration: Duration) -> String {
    if duration.is_zero() {
        "-".to_string()
    } else {
        format!("{:.1}", count as f64 / duration.as_secs_f64())
    }
}

/// What's shown of the packets as they go by.
pub struct Observer {
    printer: Option<PacketPrinter>,
    timing: Option<Timing>,
    ack_lines: bool,
}

impl Observer {
    pub fn new(dry_run: bool, timing: bool, verbose: bool, two_collections: bool) -> Self {
        Observer {
            printer: dry_run.then(|| PacketPrinter::new(verbose, two_collections)),
            timing: timing.then(Timing::default),
            ack_lines: verbose,
        }
    }

    pub fn on_event(&mut self, event: PacketEvent, at: Instant, console: &mut Console) {
        match event {
            PacketEvent::Sent { collection, report } => {
                if let Some(printer) = &mut self.printer {
                    console.lines(printer.on_sent(collection, report));
                }
                if let Some(timing) = &mut self.timing {
                    timing.on_sent(report.get(1).copied().unwrap_or(0), at);
                }
            }
            PacketEvent::Ack {
                idx,
                accepted,
                latency,
            } => {
                if let Some(timing) = &mut self.timing {
                    let line = timing.on_ack(idx, accepted, latency, at);
                    if self.ack_lines {
                        console.line(line);
                    }
                }
            }
        }
    }

    pub fn finish(&mut self, console: &mut Console) {
        if let Some(printer) = &mut self.printer {
            console.lines(printer.finish());
        }
        if let Some(timing) = &self.timing {
            console.lines(timing.summary());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::REPORT_LEN;
    use std::sync::OnceLock;

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

    /// A moment some milliseconds after a fixed start.
    fn at(ms: u64) -> Instant {
        static START: OnceLock<Instant> = OnceLock::new();
        *START.get_or_init(Instant::now) + MS(ms)
    }

    /// A start, then chunks that each take 2 ms to send and an ack wait of `waits`, then a finish.
    /// The pauses around the chunks are long, and must not show up in the figures.
    fn upload(waits: &[u64]) -> Timing {
        let mut timing = Timing::default();
        timing.on_sent(SCREEN_CONTROL, at(0));
        timing.on_ack(0, true, MS(50), at(50));
        let mut now = 50 + 120;
        for (idx, &wait) in waits.iter().enumerate() {
            now += 2;
            timing.on_sent(SCREEN_DATA, at(now));
            now += wait;
            timing.on_ack(idx as u8, true, MS(wait), at(now));
        }
        now += 75;
        timing.on_sent(SCREEN_CONTROL, at(now));
        now += 40;
        timing.on_ack(0, true, MS(40), at(now));
        timing
    }

    #[test]
    fn no_acks_say_so() {
        assert_eq!(Timing::default().summary(), ["Acks: none"]);
        let mut timing = Timing::default();
        timing.on_sent(0x0B, at(0));
        assert_eq!(timing.summary(), ["Acks: none"]);
    }

    #[test]
    fn an_upload_says_where_the_time_went() {
        // Chunk acks at 372, 674 and 776, which is 302 and 102 ms apart. The first chunk went at 172.
        let summary = upload(&[200, 300, 100]).summary();
        assert_eq!(
            summary,
            [
                "Total: 0.9 s, from the first report to the last ack",
                "Chunks: 3 sent, 3 acknowledged in 0.6 s, 5.0 per second",
                "Chunk rate: min 3.3, median 5.0, max 9.8 per second",
                "Send: min 2.0 ms, median 2.0 ms, max 2.0 ms",
                "Ack wait: min 100.0 ms, median 200.0 ms, max 300.0 ms",
                "Start and finish ack wait: min 40.0 ms, median 45.0 ms, max 50.0 ms",
            ]
        );
    }

    #[test]
    fn a_stopped_upload_still_reports_what_it_got() {
        let mut timing = Timing::default();
        timing.on_sent(SCREEN_CONTROL, at(0));
        timing.on_ack(0, true, MS(10), at(10));
        timing.on_sent(SCREEN_DATA, at(130));
        timing.on_ack(0, true, MS(20), at(150));
        timing.on_sent(SCREEN_CONTROL, at(225));
        timing.on_ack(0, true, MS(5), at(230));
        let summary = timing.summary();
        assert_eq!(
            summary,
            [
                "Total: 0.2 s, from the first report to the last ack",
                "Chunks: 1 sent, 1 acknowledged in 0.0 s, 50.0 per second",
                "Ack wait: min 20.0 ms, median 20.0 ms, max 20.0 ms",
                "Start and finish ack wait: min 5.0 ms, median 7.5 ms, max 10.0 ms",
            ]
        );
    }

    #[test]
    fn refused_acks_are_counted() {
        let mut timing = Timing::default();
        timing.on_sent(SCREEN_DATA, at(0));
        let line = timing.on_ack(7, false, MS(5), at(5));
        assert_eq!(line, "Ack 7: wait 5.0 ms, refused");
        assert_eq!(timing.summary().last().unwrap(), "Refused: 1");
    }

    #[test]
    fn an_ack_line_gives_the_send_when_there_was_a_chunk_before() {
        let mut timing = Timing::default();
        timing.on_sent(SCREEN_DATA, at(0));
        assert_eq!(timing.on_ack(0, true, MS(5), at(5)), "Ack 0: wait 5.0 ms");
        timing.on_sent(SCREEN_DATA, at(8));
        assert_eq!(
            timing.on_ack(1, true, MS(4), at(12)),
            "Ack 1: wait 4.0 ms, send 3.0 ms"
        );
    }

    #[test]
    fn no_time_passing_has_no_rate() {
        let mut timing = Timing::default();
        for _ in 0..2 {
            timing.on_sent(SCREEN_DATA, at(0));
            timing.on_ack(0, true, Duration::ZERO, at(0));
        }
        let summary = timing.summary();
        assert_eq!(
            summary[1],
            "Chunks: 2 sent, 2 acknowledged in 0.0 s, - per second"
        );
        assert_eq!(summary[2], "Chunk rate: min -, median -, max - per second");
    }

    #[test]
    fn the_median_of_an_even_count_is_the_mean_of_the_middle_two() {
        let stats = Stats::of(&[MS(40), MS(10), MS(30), MS(15)]).unwrap();
        assert_eq!(
            stats.to_string(),
            "min 10.0 ms, median 22.5 ms, max 40.0 ms"
        );
        assert!(Stats::of(&[]).is_none());
    }

    /// Runs some events through an observer, and gives what it printed.
    fn observed(observer: &mut Observer, events: &[PacketEvent]) -> String {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut console = Console {
            out: &mut out,
            err: &mut err,
        };
        for &event in events {
            observer.on_event(event, at(0), &mut console);
        }
        observer.finish(&mut console);
        assert!(err.is_empty());
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn the_observer_shows_only_what_was_asked_for() {
        let report = chunk(0, 3);
        let events = [
            PacketEvent::Sent {
                collection: Collection::Main,
                report: &report,
            },
            PacketEvent::Ack {
                idx: 3,
                accepted: true,
                latency: MS(5),
            },
        ];

        let quiet = observed(&mut Observer::new(false, false, false, false), &events);
        assert_eq!(quiet, "");

        let timing = observed(&mut Observer::new(false, true, false, false), &events);
        assert!(timing.contains("Ack wait: min 5.0 ms"), "{timing}");
        assert!(!timing.contains("Ack 3"), "{timing}");
        assert!(!timing.contains("09 0C"), "{timing}");

        let verbose = observed(&mut Observer::new(false, true, true, false), &events);
        assert!(verbose.contains("Ack 3: wait 5.0 ms"), "{verbose}");
        assert!(!verbose.contains("09 0C"), "{verbose}");

        let dry = observed(&mut Observer::new(true, false, false, false), &events);
        assert!(dry.contains("09 0C"), "{dry}");
        assert!(dry.contains("Reports: 1 x 0x0C"), "{dry}");
        assert!(!dry.contains("Ack"), "{dry}");
    }
}
