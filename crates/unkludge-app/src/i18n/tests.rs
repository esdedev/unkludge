use super::*;

#[test]
fn the_language_chooses_the_table() {
    assert_eq!(for_language(true).no_keyboard, ES.no_keyboard);
    assert_eq!(for_language(false).no_keyboard, EN.no_keyboard);
    assert_ne!(ES.no_keyboard, EN.no_keyboard);
}

#[test]
fn a_screen_reader_gets_the_caveat_on_the_chip_it_is_about() {
    for table in [&EN, &ES] {
        let name = table.sleep_chip_name(SleepTime::NoSleep);
        assert!(name.contains(table.no_sleep), "{name}");
        assert!(name.contains(table.sleep_caveat), "{name}");
        let other = table.sleep_chip_name(SleepTime::Min5);
        assert!(!other.contains(table.sleep_caveat), "{other}");
    }
}

#[test]
fn the_clock_is_only_said_to_be_sent_as_nothing_confirms_it() {
    assert_eq!(EN.clock_synced, "Clock sent");
    assert_eq!(ES.clock_synced, "Hora enviada");
}

#[test]
fn the_system_language_gives_one_of_the_tables() {
    let table = t().no_keyboard;
    assert!(table == EN.no_keyboard || table == ES.no_keyboard);
}

#[test]
fn errors_read_in_english_with_each_detail_in_its_place() {
    let cases = [
        (Error::NotFound, "No supported keyboard found"),
        (Error::Unsupported(0x01FA), "Keyboard 01FA isn't supported"),
        (
            Error::NoUsableCollection(0x0265),
            "RK S85 ES (0265) is connected, but not with the HID collections Unkludge can use",
        ),
        (Error::Hid("gone".to_string()), "USB error: gone"),
        (
            Error::ReportLengthUnknown {
                pid: 0x0265,
                detail: "no answer".to_string(),
            },
            "RK S85 ES (0265) is connected, but the system won't say how long its reports are, so \
             nothing is sent to it: no answer",
        ),
        (
            Error::NotOnUsb(0x0265),
            "RK S85 ES (0265) isn't connected by USB, and Unkludge only works by USB cable. \
             Connect it by cable",
        ),
        (
            Error::DeviceBusy,
            "Another Unkludge program is using the keyboard",
        ),
        (
            Error::LockFailed("no folder".to_string()),
            "The lock that keeps Unkludge programs off the keyboard at the same time failed: no \
             folder",
        ),
        (
            Error::StartNoAck,
            "The keyboard didn't acknowledge the start of the upload",
        ),
        (
            Error::StartRejected,
            "The keyboard refused the start of the upload",
        ),
        (
            Error::NoAck { frame: 2, idx: 7 },
            "The keyboard didn't acknowledge chunk 7 of frame 2. An abort was sent",
        ),
        (
            Error::Rejected { frame: 2, idx: 7 },
            "The keyboard refused chunk 7 of frame 2. An abort was sent",
        ),
        (Error::Cancelled, "Cancelled"),
        (
            Error::TooManyFrames { got: 61, max: 60 },
            "Too many frames: at least 61, this keyboard's limit is 60",
        ),
        (
            Error::LightsOff,
            "The lights are off: choose an effect first",
        ),
        (
            Error::CustomMode,
            "The keyboard is in the custom mode: choose an effect first",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(EN.error(&error), text);
    }
}

#[test]
fn errors_read_in_spanish_with_each_detail_in_its_place() {
    let cases = [
        (
            Error::NotFound,
            "No se ha encontrado ningún teclado compatible",
        ),
        (
            Error::Unsupported(0x01FA),
            "El teclado 01FA no es compatible",
        ),
        (
            Error::NoUsableCollection(0x0265),
            "RK S85 ES (0265) está conectado, pero no con las colecciones HID que Unkludge puede usar",
        ),
        (Error::Hid("gone".to_string()), "Error de USB: gone"),
        (
            Error::ReportLengthUnknown {
                pid: 0x0265,
                detail: "no answer".to_string(),
            },
            "RK S85 ES (0265) está conectado, pero el sistema no dice cuánto miden sus paquetes, \
             así que no se le envía nada: no answer",
        ),
        (
            Error::NotOnUsb(0x0265),
            "RK S85 ES (0265) no está conectado por USB, y Unkludge solo funciona por cable USB. \
             Conéctalo por cable",
        ),
        (
            Error::DeviceBusy,
            "Otro programa de Unkludge está usando el teclado",
        ),
        (
            Error::LockFailed("no folder".to_string()),
            "Ha fallado el bloqueo que evita que dos programas de Unkludge usen el teclado a la \
             vez: no folder",
        ),
        (
            Error::StartNoAck,
            "El teclado no confirmó el inicio de la subida",
        ),
        (
            Error::StartRejected,
            "El teclado rechazó el inicio de la subida",
        ),
        (
            Error::NoAck { frame: 2, idx: 7 },
            "El teclado no confirmó el paquete 7 del fotograma 2. Se ha enviado la orden de \
             abortar",
        ),
        (
            Error::Rejected { frame: 2, idx: 7 },
            "El teclado rechazó el paquete 7 del fotograma 2. Se ha enviado la orden de abortar",
        ),
        (Error::Cancelled, "Cancelado"),
        (
            Error::TooManyFrames { got: 61, max: 60 },
            "Demasiados fotogramas: al menos 61, y el límite de este teclado es 60",
        ),
        (
            Error::LightsOff,
            "Las luces están apagadas: elige antes un efecto",
        ),
        (
            Error::CustomMode,
            "El teclado está en el modo personalizado: elige antes un efecto",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(ES.error(&error), text);
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
    assert_eq!(ES.problem(&Problem::UnusableConfig), ES.unusable_config);
    assert_ne!(EN.unusable_config, ES.unusable_config);
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
fn cancelling_gives_the_longest_it_can_take_in_both_languages() {
    assert_eq!(
        EN.cancelling,
        "Cancelling… on a keyboard that has stopped answering this can take 16 seconds"
    );
    assert_eq!(
        ES.cancelling,
        "Cancelando… con un teclado que ha dejado de responder puede tardar 16 segundos"
    );
}

#[test]
fn notes_about_frames_fill_in_every_value() {
    assert_eq!(
        EN.frames_cut(60),
        "The file has more frames than this keyboard's limit: only the first 60 are used"
    );
    assert_eq!(
        ES.frames_cut(60),
        "El archivo tiene más fotogramas de los que admite este teclado: solo se usan los \
         primeros 60"
    );
    assert_eq!(
        EN.frames_thinned(12, 40, 10, 33),
        "At 10 ms per frame, 12 of its 40 frames are sent, as this keyboard's draw time is 33 ms"
    );
    assert_eq!(
        ES.frames_thinned(12, 40, 10, 33),
        "A 10 ms por fotograma se envían 12 de sus 40 fotogramas, porque el tiempo de dibujo de \
         este teclado es de 33 ms"
    );
}

#[test]
fn the_time_left_reads_in_both_languages() {
    assert_eq!(EN.time_left(Duration::from_secs(90)), "About 2 min left");
    assert_eq!(ES.time_left(Duration::from_secs(90)), "Quedan unos 2 min");
}

#[test]
fn an_unsupported_keyboard_is_named_when_it_is_known() {
    let known = EN.unsupported_detail(0x002A);
    assert!(known.contains("002A"), "{known}");
    assert!(known.contains("report"), "{known}");
    let unknown = ES.unsupported_detail(0xFFFF);
    assert!(unknown.contains("FFFF"), "{unknown}");
    assert!(unknown.contains("informe"), "{unknown}");
}

#[test]
fn each_table_knows_its_language() {
    assert!(for_language(true).spanish);
    assert!(!for_language(false).spanish);
}

#[test]
fn a_file_is_described_by_its_frames() {
    assert_eq!(EN.frames_info(1, 100), "Still image");
    assert_eq!(ES.frames_info(1, 100), "Imagen fija");
    assert_eq!(EN.frames_info(24, 100), "24 frames, 100 ms each");
    assert_eq!(ES.frames_info(24, 100), "24 fotogramas, 100 ms cada uno");
}

#[test]
fn a_screen_is_described_by_its_size_and_frames() {
    assert_eq!(EN.screen_info(320, 172, 60), "320 × 172, up to 60 frames");
    assert_eq!(
        ES.screen_info(240, 135, 100),
        "240 × 135, hasta 100 fotogramas"
    );
}

#[test]
fn every_effect_of_every_model_has_a_name_in_both_languages() {
    for model in unkludge_protocol::model::supported() {
        for effect in model.lighting.effects {
            // The English table is the model data's names, in the same order.
            assert_eq!(EN.effect_name(effect), effect.name);
            assert!(!ES.effect_name(effect).is_empty());
        }
    }
    let other = Effect {
        id: 30,
        name: "Later",
        color: false,
        speed: false,
        brightness: false,
        rainbow: false,
    };
    assert_eq!(ES.effect_name(&other), "Later");
    assert_eq!(ES.effect_name(&Effect { id: 0, ..other }), "Later");
}

#[test]
fn sleep_options_read_in_both_languages() {
    assert_eq!(EN.sleep_label(SleepTime::Min5), "5 min");
    assert_eq!(ES.sleep_label(SleepTime::Min5), "5 min");
    assert_eq!(EN.sleep_label(SleepTime::NoSleep), "No sleep");
    assert_eq!(ES.sleep_label(SleepTime::NoSleep), "Sin suspensión");
    assert_eq!(EN.sleep_other(0x1E), "now 0x1E");
    assert_eq!(ES.sleep_other(0x1E), "ahora 0x1E");
}

#[test]
fn colours_are_counted_from_one_for_people() {
    assert_eq!(EN.color_slot(0), "Colour slot 1");
    assert_eq!(ES.color_slot(6), "Hueco de color 7");
}

#[test]
fn a_rough_time_reads_in_both_languages() {
    assert_eq!(EN.about(Duration::from_secs(40)), "About 40 s");
    assert_eq!(ES.about(Duration::from_secs(40)), "Unos 40 s");
}

#[test]
fn durations_round_up_through_their_units() {
    let of = |seconds: f64| EN.duration(Duration::from_secs_f64(seconds));
    assert_eq!(of(0.0), "0 s");
    assert_eq!(of(0.4), "1 s");
    assert_eq!(of(59.0), "59 s");
    assert_eq!(of(59.5), "1 min");
    assert_eq!(of(60.0), "1 min");
    assert_eq!(of(61.0), "2 min");
    assert_eq!(of(90.0), "2 min");
    assert_eq!(of(3540.0), "59 min");
    assert_eq!(of(3541.0), "1 h 0 min");
    assert_eq!(of(3599.0), "1 h 0 min");
    assert_eq!(of(3600.0), "1 h 0 min");
    assert_eq!(of(3601.0), "1 h 1 min");
    assert_eq!(ES.duration(Duration::from_secs(5400)), "1 h 30 min");
}
