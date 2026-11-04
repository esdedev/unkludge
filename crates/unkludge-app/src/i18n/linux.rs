//! What only the Linux interface says. It's in both tables on every system, so that a text can't go
//! missing from either wherever this is compiled.

/// One language's texts for the Linux interface.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct LinuxTr {
    /// The button that installs the udev rule.
    pub grant_access: &'static str,
    /// Next to that button: why it's there.
    pub grant_why: &'static str,
    /// While the system's password dialog is up.
    pub granting: &'static str,
    pub access_granted: &'static str,
    /// The dialog was closed, the user isn't allowed, or there's no agent to ask.
    pub access_cancelled: &'static str,
    pub access_no_pkexec: &'static str,
    pub access_failed: &'static str,
    /// The rule is there, and the keyboard still can't be opened.
    pub access_still_denied: &'static str,
    /// There's no tray to stay in.
    pub no_tray: &'static str,
    /// The preview's hint where dropping a file can't work, which is on Wayland.
    pub click_hint: &'static str,
    /// When no program for choosing a file was found.
    pub path_title: &'static str,
    pub path_hint: &'static str,
    /// What the field for a file's path is called.
    pub path_field: &'static str,
    pub open_file: &'static str,
    pub red: &'static str,
    pub green: &'static str,
    pub blue: &'static str,
    /// What the field for a colour's code is called.
    pub color_code: &'static str,
    pub use_color: &'static str,
    pub ok: &'static str,
    /// What a screen reader calls the colour picker's preview.
    pub color_preview: &'static str,
}

pub(super) const EN: LinuxTr = LinuxTr {
    grant_access: "Grant access",
    grant_why: "Linux needs a udev rule before Unkludge can reach the keyboard",
    granting: "Waiting for the system's password dialog…",
    access_granted: "Access granted: the keyboard should show up in a moment",
    access_cancelled: "Access wasn't granted: the password dialog was closed or couldn't be shown",
    access_no_pkexec: "This system has no pkexec, so the udev rule has to be installed by hand",
    access_failed: "The udev rule couldn't be installed",
    access_still_denied: "The rule is installed and the keyboard still can't be opened: unplug it and plug it back in",
    no_tray: "This desktop has no tray to stay in, so closing only minimizes. Ctrl+Q quits",
    click_hint: "Click here to choose an image or GIF",
    path_title: "Choose an image or GIF",
    path_hint: "Neither zenity nor kdialog is installed, so the file's path has to be typed here",
    path_field: "File path",
    open_file: "Open",
    red: "Red",
    green: "Green",
    blue: "Blue",
    color_code: "Colour code",
    use_color: "Use this colour",
    ok: "OK",
    color_preview: "The colour chosen",
};

pub(super) const ES: LinuxTr = LinuxTr {
    grant_access: "Dar acceso",
    grant_why: "Linux necesita una regla udev para que Unkludge pueda llegar al teclado",
    granting: "Esperando al diálogo de contraseña del sistema…",
    access_granted: "Acceso concedido: el teclado debería aparecer en un momento",
    access_cancelled: "No se ha concedido el acceso: el diálogo de contraseña se cerró o no se pudo mostrar",
    access_no_pkexec: "Este sistema no tiene pkexec, así que la regla udev hay que instalarla a mano",
    access_failed: "No se ha podido instalar la regla udev",
    access_still_denied: "La regla está instalada y el teclado sigue sin abrirse: desenchúfalo y vuelve a enchufarlo",
    no_tray: "Este escritorio no tiene bandeja donde quedarse, así que cerrar solo minimiza. Ctrl+Q sale",
    click_hint: "Haz clic aquí para elegir una imagen o un GIF",
    path_title: "Elige una imagen o un GIF",
    path_hint: "No están instalados ni zenity ni kdialog, así que la ruta del archivo hay que escribirla aquí",
    path_field: "Ruta del archivo",
    open_file: "Abrir",
    red: "Rojo",
    green: "Verde",
    blue: "Azul",
    color_code: "Código de color",
    use_color: "Usar este color",
    ok: "Aceptar",
    color_preview: "El color elegido",
};

#[cfg(target_os = "linux")]
impl LinuxTr {
    /// How asking for access went.
    pub fn grant_result(&self, result: Result<(), crate::sys::GrantError>) -> &'static str {
        use crate::sys::GrantError;
        match result {
            Ok(()) => self.access_granted,
            Err(GrantError::NoPkexec) => self.access_no_pkexec,
            Err(GrantError::Cancelled) => self.access_cancelled,
            Err(GrantError::Failed) => self.access_failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(table: &LinuxTr) -> Vec<&'static str> {
        vec![
            table.grant_access,
            table.grant_why,
            table.granting,
            table.access_granted,
            table.access_cancelled,
            table.access_no_pkexec,
            table.access_failed,
            table.access_still_denied,
            table.no_tray,
            table.click_hint,
            table.path_title,
            table.path_hint,
            table.path_field,
            table.open_file,
            table.red,
            table.green,
            table.blue,
            table.color_code,
            table.use_color,
            table.ok,
            table.color_preview,
        ]
    }

    #[test]
    fn every_text_is_there_and_translated() {
        for (en, es) in texts(&EN).iter().zip(texts(&ES)) {
            assert!(!en.is_empty() && !es.is_empty());
            assert_ne!(*en, es);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn each_way_of_asking_for_access_ending_has_its_own_words() {
        use crate::sys::GrantError;
        for table in [&EN, &ES] {
            let results = [
                table.grant_result(Ok(())),
                table.grant_result(Err(GrantError::NoPkexec)),
                table.grant_result(Err(GrantError::Cancelled)),
                table.grant_result(Err(GrantError::Failed)),
            ];
            for (i, text) in results.iter().enumerate() {
                assert!(!results[i + 1..].contains(text), "{text}");
            }
        }
        assert_eq!(EN.grant_result(Ok(())), EN.access_granted);
        assert_eq!(ES.grant_result(Err(GrantError::Failed)), ES.access_failed);
    }
}
