//! What the app says, in English and Spanish. Both tables are the same struct, so a text can't be
//! missing from one of them. The protocol crate's own messages are in English: the common ones get
//! translated in [`Tr::error`], and those that come with details of their own go through as they are.

use std::sync::OnceLock;
use std::time::Duration;

use unkludge_protocol::Error;
use unkludge_protocol::config::SleepTime;
use unkludge_protocol::model::Effect;
use unkludge_protocol::screen::Fit;

use crate::worker::Problem;

/// One language's texts. In the private ones, `{name}` marks where a value goes.
pub struct Tr {
    /// Which language this is, for what needs to know, as text shaping does.
    pub spanish: bool,
    /// Shown when there's no keyboard.
    pub connect_keyboard: &'static str,
    /// [`Problem::NoKeyboard`].
    pub no_keyboard: &'static str,
    /// [`Problem::Busy`].
    pub busy: &'static str,
    /// [`Problem::UnusableConfig`].
    pub unusable_config: &'static str,
    /// While [`crate::worker::Event::RkAppRunning`] says so.
    pub rk_app_warning: &'static str,
    /// Before [`crate::worker::Event::CannotOpen`]'s error.
    pub cannot_open: &'static str,
    /// From [`crate::worker::Event::Hung`] to [`crate::worker::Event::Responding`].
    pub hung: &'static str,
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

    /// Under the app's name when no model is showing.
    pub tagline: &'static str,
    /// A keyboard is there that this version can't use.
    pub unsupported_title: &'static str,
    pub unsupported_short: &'static str,
    /// While the settings file can't be read.
    pub settings_unreadable: &'static str,
    /// The first time the window is closed.
    pub tray_note_title: &'static str,
    pub tray_note: &'static str,
    /// What goes before a choice when a screen reader names it.
    pub effect: &'static str,
    pub fit: &'static str,
    /// What a screen reader calls the automatic clock's toggle, which is read away from its card.
    pub auto_clock_name: &'static str,
    /// The title of the message about a mistyped option.
    pub bad_option: &'static str,
    pub looking: &'static str,
    pub connected: &'static str,
    pub disconnected: &'static str,
    pub not_responding: &'static str,
    /// [`Self::cannot_open`], in the little room of the status at the top.
    pub cannot_open_short: &'static str,
    pub experimental: &'static str,
    pub send_report: &'static str,
    /// The report was saved and its folder opened.
    pub report_saved: &'static str,
    /// Before why a report couldn't be saved.
    pub report_failed: &'static str,
    /// Under [`Self::connect_keyboard`].
    pub connect_hint: &'static str,
    /// In the window's title and its label.
    pub demo: &'static str,
    /// What the demo label stands for, for screen readers.
    pub demo_name: &'static str,

    pub screen: &'static str,
    pub drop_hint: &'static str,
    pub drop_hint_more: &'static str,
    pub choose_file: &'static str,
    pub no_file: &'static str,
    /// The file dialog's title.
    pub choose_title: &'static str,
    /// The kind of file the dialog's filter lets through.
    pub image_files: &'static str,
    pub reading_file: &'static str,
    pub still_image: &'static str,
    pub fit_cover: &'static str,
    pub fit_contain: &'static str,
    pub fit_stretch: &'static str,
    pub frame_time: &'static str,
    pub slower: &'static str,
    pub faster: &'static str,
    pub upload: &'static str,
    pub uploading: &'static str,
    pub cancel: &'static str,

    pub lights: &'static str,
    pub off: &'static str,
    pub brightness: &'static str,
    pub speed: &'static str,
    pub color: &'static str,
    pub rainbow: &'static str,
    pub custom_color: &'static str,
    pub own_colors: &'static str,
    pub lights_off_note: &'static str,
    pub custom_mode_note: &'static str,
    pub unknown_effect_note: &'static str,
    pub sleep: &'static str,
    pub no_sleep: &'static str,
    /// The colour dialog comes after this, once per model.
    pub first_custom_title: &'static str,
    pub first_custom_warning: &'static str,

    pub clock: &'static str,
    pub sync_clock: &'static str,
    pub auto_clock: &'static str,
    pub auto_clock_more: &'static str,

    pub start_with_system: &'static str,
    pub tray_open: &'static str,
    pub tray_sync_clock: &'static str,
    pub tray_quit: &'static str,
    /// What a screen reader calls the preview.
    pub preview: &'static str,

    /// By effect number, from 1. The model data's own names are the English ones.
    effects: [&'static str; 17],
    frames_info: &'static str,
    file_waiting: &'static str,
    unsupported_known: &'static str,
    unsupported_unknown: &'static str,
    screen_info: &'static str,
    sleep_other: &'static str,
    about: &'static str,
    /// How a screen reader names each colour.
    color_slot: &'static str,

    frames_cut: &'static str,
    frames_thinned: &'static str,
    time_left: &'static str,

    err_not_found: &'static str,
    err_unsupported: &'static str,
    err_usb: &'static str,
    err_start_no_ack: &'static str,
    err_start_rejected: &'static str,
    err_no_ack: &'static str,
    err_rejected: &'static str,
    err_cancelled: &'static str,
    err_too_many_frames: &'static str,
    err_lights_off: &'static str,
    err_custom_mode: &'static str,
}

const EN: Tr = Tr {
    spanish: false,
    connect_keyboard: "Connect the keyboard by USB cable",
    no_keyboard: "No keyboard is connected",
    busy: "Not possible during an upload: wait for it to end, or cancel it",
    unusable_config: "This keyboard's settings can't be read properly, so nothing gets written to it",
    rk_app_warning: "DeviceDriver.exe, which may be RK's app, is running. It polls the keyboard and \
                     sends its own settings back, so close it first",
    cannot_open: "The keyboard can't be opened",
    hung: "The keyboard isn't answering: unplug it and plug it back in",
    cancelling: "Cancelling… on a keyboard that has stopped answering this can take 16 seconds",
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

    tagline: "Royal Kludge keyboards, without the bloat",
    unsupported_title: "This keyboard isn't supported yet",
    unsupported_short: "Not supported",
    settings_unreadable: "The settings file can't be read, so these are the defaults, and changes won't be saved",
    tray_note_title: "Unkludge keeps running",
    tray_note: "It stays here, in the notification area, to keep the keyboard's clock right. Right-click the icon to quit.",
    effect: "Effect",
    fit: "Fit",
    auto_clock_name: "Set the clock automatically",
    bad_option: "Unkludge didn't start",
    looking: "Looking for a keyboard…",
    connected: "Connected",
    disconnected: "No keyboard",
    not_responding: "Not responding",
    cannot_open_short: "Can't open it",
    experimental: "EXPERIMENTAL",
    send_report: "Send report",
    report_saved: "Report saved: send the file in the folder that just opened",
    report_failed: "The report couldn't be saved",
    connect_hint: "It shows up here by itself as soon as it's plugged in",
    demo: "DEMO",
    demo_name: "Demo: no real keyboard is used",

    screen: "Screen",
    drop_hint: "Drop an image or GIF here",
    drop_hint_more: "or click to choose one",
    choose_file: "Choose file…",
    no_file: "No file chosen",
    choose_title: "Choose an image or GIF",
    image_files: "Images and GIFs",
    reading_file: "Reading the file…",
    still_image: "Still image",
    fit_cover: "Crop",
    fit_contain: "Fit",
    fit_stretch: "Stretch",
    frame_time: "Time per frame",
    slower: "Slower",
    faster: "Faster",
    upload: "Upload to keyboard",
    uploading: "Uploading",
    cancel: "Cancel",

    lights: "Lights",
    off: "Off",
    brightness: "Brightness",
    speed: "Speed",
    color: "Colour",
    rainbow: "Rainbow",
    custom_color: "Custom colour…",
    own_colors: "This effect has colours of its own",
    lights_off_note: "The lights are off: choose an effect to turn them on",
    custom_mode_note: "The keyboard is in its custom mode, set with another tool: choosing an effect leaves it",
    unknown_effect_note: "The keyboard is showing an effect Unkludge doesn't know: choosing one replaces it",
    sleep: "Sleep",
    no_sleep: "No sleep",
    first_custom_title: "First custom colour",
    first_custom_warning: "The keyboard can't say what colours it holds, and a custom colour is written together with the colours of every effect. So this first time, any colours you set on other effects with another app go back to Unkludge's defaults. Unkludge remembers them from then on.",

    clock: "Clock",
    sync_clock: "Sync clock now",
    auto_clock: "Set it automatically",
    auto_clock_more: "On start, on connecting and after sleep",

    start_with_system: "Start with the system",
    tray_open: "Open",
    tray_sync_clock: "Sync clock",
    tray_quit: "Quit",
    preview: "Preview of the screen",

    effects: [
        "Solid",
        "Breathing",
        "Rainbow",
        "Flash",
        "Raindrops",
        "Colour wheel",
        "Ripples",
        "Starlight",
        "Fade",
        "Snake",
        "Neon",
        "Reactive",
        "Sine wave",
        "Scanner",
        "Windmill",
        "Waterfall",
        "Blossom",
    ],
    frames_info: "{frames} frames, {ms} ms each",
    file_waiting: "{name} will load when the keyboard is connected",
    unsupported_known: "{name} (PID {pid}) can't be used with this version. Send a report so it can be looked at",
    unsupported_unknown: "A device with PID {pid} isn't one this version knows. If it's your keyboard, send a report so it can be looked at",
    screen_info: "{width} × {height}, up to {frames} frames",
    sleep_other: "now {raw}",
    about: "About {time}",
    color_slot: "Colour slot {slot}",

    err_not_found: "No supported keyboard found",
    err_unsupported: "Keyboard {pid} isn't supported",
    err_usb: "USB error: {detail}",
    err_start_no_ack: "The keyboard didn't acknowledge the start of the upload",
    err_start_rejected: "The keyboard refused the start of the upload",
    err_no_ack: "The keyboard didn't acknowledge chunk {idx} of frame {frame}. An abort was sent",
    err_rejected: "The keyboard refused chunk {idx} of frame {frame}. An abort was sent",
    err_cancelled: "Cancelled",
    err_too_many_frames: "Too many frames: at least {got}, this keyboard's limit is {max}",
    err_lights_off: "The lights are off: choose an effect first",
    err_custom_mode: "The keyboard is in the custom mode: choose an effect first",
};

const ES: Tr = Tr {
    spanish: true,
    connect_keyboard: "Conecta el teclado por cable USB",
    no_keyboard: "No hay ningún teclado conectado",
    busy: "No se puede durante una subida: espera a que termine o cancélala",
    unusable_config: "Los ajustes de este teclado no se leen bien, así que no se le escribe nada",
    rk_app_warning: "DeviceDriver.exe, que puede ser el programa de RK, está en marcha. Ese programa \
                     sondea el teclado y le vuelve a enviar sus ajustes, así que ciérralo antes",
    cannot_open: "No se puede abrir el teclado",
    hung: "El teclado no responde: desenchúfalo y vuelve a enchufarlo",
    cancelling: "Cancelando… con un teclado que ha dejado de responder puede tardar 16 segundos",
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

    tagline: "Teclados Royal Kludge, sin lo que sobra",
    unsupported_title: "Este teclado aún no es compatible",
    unsupported_short: "No compatible",
    settings_unreadable: "No se puede leer el archivo de ajustes, así que estos son los predeterminados y los cambios no se guardarán",
    tray_note_title: "Unkludge sigue en marcha",
    tray_note: "Se queda aquí, en el área de notificación, para mantener en hora el reloj del teclado. Haz clic derecho en el icono para salir.",
    effect: "Efecto",
    fit: "Encaje",
    auto_clock_name: "Ajustar la hora automáticamente",
    bad_option: "Unkludge no ha arrancado",
    looking: "Buscando un teclado…",
    connected: "Conectado",
    disconnected: "Sin teclado",
    not_responding: "No responde",
    cannot_open_short: "No se puede abrir",
    experimental: "EXPERIMENTAL",
    send_report: "Enviar informe",
    report_saved: "Informe guardado: envía el archivo de la carpeta que se acaba de abrir",
    report_failed: "No se ha podido guardar el informe",
    connect_hint: "Aparecerá aquí en cuanto lo enchufes",
    demo: "DEMO",
    demo_name: "Demostración: no se usa ningún teclado real",

    screen: "Pantalla",
    drop_hint: "Arrastra aquí una imagen o un GIF",
    drop_hint_more: "o haz clic para elegirlo",
    choose_file: "Elegir archivo…",
    no_file: "Ningún archivo elegido",
    choose_title: "Elige una imagen o un GIF",
    image_files: "Imágenes y GIF",
    reading_file: "Leyendo el archivo…",
    still_image: "Imagen fija",
    fit_cover: "Recortar",
    fit_contain: "Encajar",
    fit_stretch: "Estirar",
    frame_time: "Tiempo por fotograma",
    slower: "Más lento",
    faster: "Más rápido",
    upload: "Subir al teclado",
    uploading: "Subiendo",
    cancel: "Cancelar",

    lights: "Luces",
    off: "Apagadas",
    brightness: "Brillo",
    speed: "Velocidad",
    color: "Color",
    rainbow: "Arcoíris",
    custom_color: "Color propio…",
    own_colors: "Este efecto tiene sus propios colores",
    lights_off_note: "Las luces están apagadas: elige un efecto para encenderlas",
    custom_mode_note: "El teclado está en su modo personalizado, puesto con otra herramienta: al elegir un efecto se sale de él",
    unknown_effect_note: "El teclado muestra un efecto que Unkludge no conoce: al elegir uno se sustituye",
    sleep: "Suspensión",
    no_sleep: "Sin suspensión",
    first_custom_title: "Primer color propio",
    first_custom_warning: "El teclado no puede decir qué colores tiene, y un color propio se escribe junto con los de todos los efectos. Así que esta primera vez, los colores que hayas puesto a otros efectos con otro programa vuelven a los predeterminados de Unkludge. A partir de entonces, Unkludge los recuerda.",

    clock: "Reloj",
    sync_clock: "Sincronizar hora ahora",
    auto_clock: "Ajustarla automáticamente",
    auto_clock_more: "Al arrancar, al conectar y al volver de la suspensión",

    start_with_system: "Arrancar con el sistema",
    tray_open: "Abrir",
    tray_sync_clock: "Sincronizar hora",
    tray_quit: "Salir",
    preview: "Vista previa de la pantalla",

    effects: [
        "Fijo",
        "Respiración",
        "Arcoíris",
        "Destello",
        "Gotas de lluvia",
        "Rueda de color",
        "Ondas",
        "Luz de estrellas",
        "Fundido",
        "Serpiente",
        "Neón",
        "Reactivo",
        "Onda senoidal",
        "Escáner",
        "Molino",
        "Cascada",
        "Flor",
    ],
    frames_info: "{frames} fotogramas, {ms} ms cada uno",
    file_waiting: "{name} se cargará cuando conectes el teclado",
    unsupported_known: "{name} (PID {pid}) no se puede usar con esta versión. Envía un informe para que se pueda estudiar",
    unsupported_unknown: "Esta versión no conoce el dispositivo con PID {pid}. Si es tu teclado, envía un informe para que se pueda estudiar",
    screen_info: "{width} × {height}, hasta {frames} fotogramas",
    sleep_other: "ahora {raw}",
    about: "Unos {time}",
    color_slot: "Hueco de color {slot}",

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
    err_lights_off: "Las luces están apagadas: elige antes un efecto",
    err_custom_mode: "El teclado está en el modo personalizado: elige antes un efecto",
};

static TABLE: OnceLock<&'static Tr> = OnceLock::new();

/// The table for the system's language, unless [`choose`] said otherwise.
pub fn t() -> &'static Tr {
    TABLE.get_or_init(|| for_language(crate::sys::is_spanish()))
}

/// Uses a language whatever the system's. Must come before the first [`t`].
pub fn choose(spanish: bool) {
    let _ = TABLE.set(for_language(spanish));
}

/// Spanish if the language is Spanish, and English otherwise.
fn for_language(spanish: bool) -> &'static Tr {
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
            Error::LightsOff => self.err_lights_off.to_string(),
            Error::CustomMode => self.err_custom_mode.to_string(),
            Error::Invalid(text) | Error::Image(text) => text.clone(),
        }
    }

    pub fn problem(&self, problem: &Problem) -> String {
        match problem {
            Problem::Busy => self.busy.to_string(),
            Problem::NoKeyboard => self.no_keyboard.to_string(),
            Problem::UnusableConfig => self.unusable_config.to_string(),
            Problem::Device(error) => self.error(error),
        }
    }

    /// For a file with more frames than the keyboard's limit, of which `frames` get used.
    pub fn frames_cut(&self, frames: usize) -> String {
        self.frames_cut.replace("{frames}", &frames.to_string())
    }

    /// For an animation faster than the keyboard's draw time, of whose `frames` only `sent` go out.
    pub fn frames_thinned(
        &self,
        sent: usize,
        frames: usize,
        delay_ms: u16,
        draw_ms: u16,
    ) -> String {
        self.frames_thinned
            .replace("{sent}", &sent.to_string())
            .replace("{frames}", &frames.to_string())
            .replace("{delay}", &delay_ms.to_string())
            .replace("{draw}", &draw_ms.to_string())
    }

    pub fn time_left(&self, time: Duration) -> String {
        self.time_left.replace("{time}", &self.duration(time))
    }

    /// How long an upload may take, before it starts.
    pub fn about(&self, time: Duration) -> String {
        self.about.replace("{time}", &self.duration(time))
    }

    /// What a file holds: a still image, or how many frames of how long.
    pub fn frames_info(&self, frames: usize, delay_ms: u16) -> String {
        if frames < 2 {
            return self.still_image.to_string();
        }
        self.frames_info
            .replace("{frames}", &frames.to_string())
            .replace("{ms}", &delay_ms.to_string())
    }

    /// A file waiting for a keyboard.
    pub fn file_waiting(&self, name: &str) -> String {
        self.file_waiting.replace("{name}", name)
    }

    /// What to say about a device this version can't use, and what to do next.
    pub fn unsupported_detail(&self, pid: u16) -> String {
        let pid_text = format!("{pid:04X}");
        match unkludge_protocol::model::find(pid) {
            Some(model) => self
                .unsupported_known
                .replace("{name}", model.name)
                .replace("{pid}", &pid_text),
            None => self.unsupported_unknown.replace("{pid}", &pid_text),
        }
    }

    /// A choice with what it's a choice of, for a screen reader, which reads it on its own.
    pub fn labelled(&self, label: &str, value: &str) -> String {
        format!("{label}: {value}")
    }

    /// What a model's screen is like.
    pub fn screen_info(&self, width: u16, height: u16, frames: u16) -> String {
        self.screen_info
            .replace("{width}", &width.to_string())
            .replace("{height}", &height.to_string())
            .replace("{frames}", &frames.to_string())
    }

    /// An effect's name in this language. A number with no translation keeps the model's name.
    pub fn effect_name(&self, effect: &Effect) -> &'static str {
        usize::from(effect.id)
            .checked_sub(1)
            .and_then(|at| self.effects.get(at))
            .copied()
            .unwrap_or(effect.name)
    }

    pub fn sleep_label(&self, sleep: SleepTime) -> &'static str {
        match sleep {
            SleepTime::NoSleep => self.no_sleep,
            other => other.label(),
        }
    }

    pub fn fit_name(&self, fit: Fit) -> &'static str {
        match fit {
            Fit::Cover => self.fit_cover,
            Fit::Contain => self.fit_contain,
            Fit::Stretch => self.fit_stretch,
        }
    }

    /// A sleep value the keyboard has that isn't one of the options.
    pub fn sleep_other(&self, raw: u8) -> String {
        self.sleep_other.replace("{raw}", &format!("0x{raw:02X}"))
    }

    /// One of an effect's colours, counted from 1.
    pub fn color_slot(&self, slot: u8) -> String {
        self.color_slot
            .replace("{slot}", &(u16::from(slot) + 1).to_string())
    }

    /// A time, rounded up to its smallest unit, seconds under a minute and minutes above that:
    /// `40 s`, `3 min`, `1 h 20 min`. These units read the same in both languages.
    pub fn duration(&self, duration: Duration) -> String {
        let seconds = duration.as_secs() + u64::from(duration.subsec_nanos() > 0);
        if seconds < 60 {
            return format!("{seconds} s");
        }
        let minutes = seconds.div_ceil(60);
        if minutes < 60 {
            format!("{minutes} min")
        } else {
            format!("{} h {} min", minutes / 60, minutes % 60)
        }
    }
}

fn chunk_text(template: &str, frame: u8, idx: u8) -> String {
    template
        .replace("{frame}", &frame.to_string())
        .replace("{idx}", &idx.to_string())
}

#[cfg(test)]
mod tests;
