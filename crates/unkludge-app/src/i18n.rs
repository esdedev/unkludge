//! What the app says, in English and Spanish. Both tables are the same struct, so a text can't be
//! missing from one of them. The protocol crate's own messages are in English: the common ones get
//! translated in [`Tr::error`], and those that come with details of their own go through as they are.

use std::time::Duration;

use unkludge_protocol::Error;

use crate::worker::Problem;

/// One language's texts. `{name}` marks where a value goes.
pub struct Tr {
    /// Shown when there's no keyboard.
    pub connect_keyboard: &'static str,
    /// [`Problem::NoKeyboard`].
    pub no_keyboard: &'static str,
    /// [`Problem::Busy`].
    pub busy: &'static str,
    /// While [`crate::worker::Event::RkAppRunning`] says so.
    pub rk_app_warning: &'static str,
    /// Before [`crate::worker::Event::CannotOpen`]'s error.
    pub cannot_open: &'static str,
    /// [`crate::worker::Event::Cancelling`].
    pub cancelling: &'static str,
    /// [`crate::worker::Event::UploadDone`].
    pub upload_done: &'static str,
    /// [`crate::worker::Event::UploadUnconfirmed`].
    pub upload_unconfirmed: &'static str,
    /// [`crate::worker::Event::UploadCancelled`].
    pub upload_cancelled: &'static str,
    /// Before [`crate::worker::Event::UploadFailed`]'s error.
    pub upload_failed: &'static str,
    /// Before [`crate::worker::Event::ImageFailed`]'s error.
    pub image_failed: &'static str,
    /// [`crate::worker::Event::ClockSynced`].
    pub clock_synced: &'static str,
    /// `{frames}`: the frames kept, from a file that has more than the keyboard's limit.
    pub frames_cut: &'static str,
    /// `{sent}`: the frames sent, `{frames}`: those in the file, `{delay}`: the delay in ms,
    /// `{draw}`: the keyboard's draw time in ms.
    pub frames_thinned: &'static str,
    /// `{time}`: from [`Tr::duration`].
    pub time_left: &'static str,

    err_not_found: &'static str,
    /// `{pid}`
    err_unsupported: &'static str,
    /// `{detail}`
    err_usb: &'static str,
    err_start_no_ack: &'static str,
    err_start_rejected: &'static str,
    /// `{idx}`, `{frame}`
    err_no_ack: &'static str,
    /// `{idx}`, `{frame}`
    err_rejected: &'static str,
    err_cancelled: &'static str,
    /// `{got}`, `{max}`
    err_too_many_frames: &'static str,
}

const EN: Tr = Tr {
    connect_keyboard: "Connect the keyboard by USB cable",
    no_keyboard: "No keyboard is connected",
    busy: "Not possible during an upload: wait for it to end, or cancel it",
    rk_app_warning: "DeviceDriver.exe, which may be RK's app, is running. It polls the keyboard and \
                     sends its own settings back, so close it first",
    cannot_open: "The keyboard can't be opened",
    cancelling: "Cancelling… on a keyboard that has stopped answering this can take 15 seconds",
    upload_done: "Uploaded",
    upload_unconfirmed: "The keyboard took every frame but didn't confirm the end, so it may not \
                         show the new animation",
    upload_cancelled: "Upload cancelled",
    upload_failed: "The upload failed",
    image_failed: "That file can't be used",
    clock_synced: "Clock set",
    frames_cut: "The file has more frames than this keyboard's limit: only the first {frames} are \
                 used",
    frames_thinned: "At {delay} ms per frame, {sent} of its {frames} frames are sent, as this \
                     keyboard's draw time is {draw} ms",
    time_left: "About {time} left",

    err_not_found: "No supported keyboard found",
    err_unsupported: "Keyboard {pid} isn't supported",
    err_usb: "USB error: {detail}",
    err_start_no_ack: "The keyboard didn't acknowledge the start of the upload",
    err_start_rejected: "The keyboard refused the start of the upload",
    err_no_ack: "The keyboard didn't acknowledge chunk {idx} of frame {frame}. An abort was sent",
    err_rejected: "The keyboard refused chunk {idx} of frame {frame}. An abort was sent",
    err_cancelled: "Cancelled",
    err_too_many_frames: "Too many frames: at least {got}, this keyboard's limit is {max}",
};

const ES: Tr = Tr {
    connect_keyboard: "Conecta el teclado por cable USB",
    no_keyboard: "No hay ningún teclado conectado",
    busy: "No se puede durante una subida: espera a que termine o cancélala",
    rk_app_warning: "DeviceDriver.exe, que puede ser el programa de RK, está en marcha. Ese programa \
                     sondea el teclado y le vuelve a enviar sus ajustes, así que ciérralo antes",
    cannot_open: "No se puede abrir el teclado",
    cancelling: "Cancelando… con un teclado que ha dejado de responder puede tardar 15 segundos",
    upload_done: "Subida terminada",
    upload_unconfirmed: "El teclado recibió todos los fotogramas pero no confirmó el final, así que \
                         puede que no muestre la animación nueva",
    upload_cancelled: "Subida cancelada",
    upload_failed: "La subida ha fallado",
    image_failed: "No se puede usar ese archivo",
    clock_synced: "Hora ajustada",
    frames_cut: "El archivo tiene más fotogramas de los que admite este teclado: solo se usan los \
                 primeros {frames}",
    frames_thinned: "A {delay} ms por fotograma se envían {sent} de sus {frames} fotogramas, porque \
                     el tiempo de dibujo de este teclado es de {draw} ms",
    time_left: "Quedan unos {time}",

    err_not_found: "No se ha encontrado ningún teclado compatible",
    err_unsupported: "El teclado {pid} no es compatible",
    err_usb: "Error de USB: {detail}",
    err_start_no_ack: "El teclado no confirmó el inicio de la subida",
    err_start_rejected: "El teclado rechazó el inicio de la subida",
    err_no_ack: "El teclado no confirmó el paquete {idx} del fotograma {frame}. Se ha enviado la \
                 orden de abortar",
    err_rejected: "El teclado rechazó el paquete {idx} del fotograma {frame}. Se ha enviado la \
                   orden de abortar",
    err_cancelled: "Cancelado",
    err_too_many_frames: "Demasiados fotogramas: al menos {got}, y el límite de este teclado es {max}",
};

/// The table for the system's language: Spanish if that's Spanish, English otherwise.
pub fn for_language(spanish: bool) -> &'static Tr {
    if spanish { &ES } else { &EN }
}

impl Tr {
    /// An error from the protocol crate, in this language where there's a translation.
    pub fn error(&self, error: &Error) -> String {
        match error {
            Error::NotFound => self.err_not_found.to_string(),
            Error::Unsupported(pid) => self.err_unsupported.replace("{pid}", &format!("{pid:04X}")),
            Error::Hid(detail) => self.err_usb.replace("{detail}", detail),
            Error::StartNoAck => self.err_start_no_ack.to_string(),
            Error::StartRejected => self.err_start_rejected.to_string(),
            Error::NoAck { frame, idx } => chunk_text(self.err_no_ack, *frame, *idx),
            Error::Rejected { frame, idx } => chunk_text(self.err_rejected, *frame, *idx),
            Error::Cancelled => self.err_cancelled.to_string(),
            Error::TooManyFrames { got, max } => self
                .err_too_many_frames
                .replace("{got}", &got.to_string())
                .replace("{max}", &max.to_string()),
            Error::Invalid(text) | Error::Image(text) => text.clone(),
        }
    }

    pub fn problem(&self, problem: &Problem) -> String {
        match problem {
            Problem::Busy => self.busy.to_string(),
            Problem::NoKeyboard => self.no_keyboard.to_string(),
            Problem::Device(error) => self.error(error),
        }
    }

    /// A time, rounded up: `40 s`, `3 min`, `1 h 20 min`. All these units read the same in both
    /// languages.
    pub fn duration(&self, duration: Duration) -> String {
        let seconds = duration.as_secs() + u64::from(duration.subsec_nanos() > 0);
        match seconds {
            0..60 => format!("{seconds} s"),
            60..3600 => format!("{} min", seconds.div_ceil(60)),
            _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
        }
    }
}

fn chunk_text(template: &str, frame: u8, idx: u8) -> String {
    template
        .replace("{frame}", &frame.to_string())
        .replace("{idx}", &idx.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn all_errors() -> Vec<Error> {
        vec![
            Error::NotFound,
            Error::Unsupported(0x01FA),
            Error::Hid("device gone".to_string()),
            Error::StartNoAck,
            Error::StartRejected,
            Error::NoAck { frame: 2, idx: 7 },
            Error::Rejected { frame: 2, idx: 7 },
            Error::Invalid("bad value".to_string()),
            Error::Cancelled,
            Error::Image("bad file".to_string()),
            Error::TooManyFrames { got: 61, max: 60 },
        ]
    }

    #[test]
    fn the_language_chooses_the_table() {
        assert_eq!(for_language(true).no_keyboard, ES.no_keyboard);
        assert_eq!(for_language(false).no_keyboard, EN.no_keyboard);
        assert_ne!(ES.no_keyboard, EN.no_keyboard);
    }

    #[test]
    fn every_error_has_words_with_its_details_filled_in() {
        for table in [&EN, &ES] {
            for error in all_errors() {
                let text = table.error(&error);
                assert!(!text.is_empty(), "{error:?}");
                assert!(!text.contains('{'), "{text}");
            }
            assert!(table.error(&Error::Unsupported(0x01FA)).contains("01FA"));
            let usb = table.error(&Error::Hid("device gone".into()));
            assert!(usb.contains("device gone"), "{usb}");
            let no_ack = table.error(&Error::NoAck { frame: 2, idx: 7 });
            assert!(no_ack.contains('2') && no_ack.contains('7'), "{no_ack}");
            let rejected = table.error(&Error::Rejected { frame: 2, idx: 7 });
            assert!(
                rejected.contains('2') && rejected.contains('7'),
                "{rejected}"
            );
            let many = table.error(&Error::TooManyFrames { got: 61, max: 60 });
            assert!(many.contains("61") && many.contains("60"), "{many}");
        }
    }

    #[test]
    fn the_common_errors_are_translated() {
        for error in all_errors() {
            let translated = !matches!(error, Error::Invalid(_) | Error::Image(_));
            assert_eq!(
                EN.error(&error) != ES.error(&error),
                translated,
                "{error:?}"
            );
        }
    }

    #[test]
    fn what_has_no_translation_passes_through() {
        for table in [&EN, &ES] {
            assert_eq!(
                table.error(&Error::Invalid("bad value".into())),
                "bad value"
            );
            assert_eq!(table.error(&Error::Image("bad file".into())), "bad file");
        }
    }

    #[test]
    fn problems_have_words() {
        assert_eq!(EN.problem(&Problem::Busy), EN.busy);
        assert_eq!(ES.problem(&Problem::Busy), ES.busy);
        assert_eq!(ES.problem(&Problem::NoKeyboard), ES.no_keyboard);
        assert_eq!(
            ES.problem(&Problem::Device(Error::StartNoAck)),
            ES.error(&Error::StartNoAck)
        );
    }

    #[test]
    fn the_rk_app_warning_only_says_it_may_be_that() {
        assert!(EN.rk_app_warning.contains("DeviceDriver.exe"));
        assert!(EN.rk_app_warning.contains("may be"));
        assert!(ES.rk_app_warning.contains("DeviceDriver.exe"));
        assert!(ES.rk_app_warning.contains("puede"));
    }

    #[test]
    fn durations_read_as_seconds_then_minutes_then_hours() {
        assert_eq!(EN.duration(Duration::from_millis(400)), "1 s");
        assert_eq!(EN.duration(Duration::from_secs(59)), "59 s");
        assert_eq!(EN.duration(Duration::from_secs(60)), "1 min");
        assert_eq!(EN.duration(Duration::from_secs(90)), "2 min");
        assert_eq!(EN.duration(Duration::from_secs(3599)), "60 min");
        assert_eq!(EN.duration(Duration::from_secs(3600)), "1 h 0 min");
        assert_eq!(ES.duration(Duration::from_secs(5400)), "1 h 30 min");
    }
}
