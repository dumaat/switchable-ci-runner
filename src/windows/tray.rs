//! The tray icon and its menu. No window: the menu is the whole interface.
use std::os::windows::process::CommandExt;

use switchable_ci_runner::decision::{Mode, Reason};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub enum Action {
    SetMode(Mode),
    SetBatterySendsToGithub(bool),
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    automatic: CheckMenuItem,
    disabled: CheckMenuItem,
    battery: CheckMenuItem,
    quit: MenuId,
}

impl Tray {
    pub fn new(mode: Mode, battery_sends_to_github: bool) -> Result<Self, String> {
        let status = MenuItem::new("Starting…", false, None);
        let automatic = CheckMenuItem::new(
            "Automatic (run jobs when idle 5 min)",
            true,
            mode == Mode::Automatic,
            None,
        );
        let disabled = CheckMenuItem::new(
            "Disabled (never run jobs)",
            true,
            mode == Mode::Disabled,
            None,
        );
        let battery = CheckMenuItem::new(
            "On battery, send jobs to GitHub",
            true,
            battery_sends_to_github,
            None,
        );
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &automatic,
            &disabled,
            &PredefinedMenuItem::separator(),
            &battery,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| format!("tray menu: {e}"))?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("switchable-ci-runner")
            .with_icon(dot(Color::Gray)?)
            .build()
            .map_err(|e| format!("tray icon: {e}"))?;

        Ok(Tray {
            icon,
            status,
            automatic,
            disabled,
            battery,
            quit: quit.id().clone(),
        })
    }

    /// Menu clicks since the last call, as actions; the checkmarks are kept consistent here.
    pub fn actions(&self) -> Vec<Action> {
        let mut actions = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *self.automatic.id() {
                self.show_mode(Mode::Automatic);
                actions.push(Action::SetMode(Mode::Automatic));
            } else if event.id == *self.disabled.id() {
                self.show_mode(Mode::Disabled);
                actions.push(Action::SetMode(Mode::Disabled));
            } else if event.id == *self.battery.id() {
                actions.push(Action::SetBatterySendsToGithub(self.battery.is_checked()));
            } else if event.id == self.quit {
                actions.push(Action::Quit);
            }
        }
        actions
    }

    /// A Windows balloon (toast on Windows 10 and 11). tray-icon has none, so a
    /// short hidden PowerShell shows it with .NET's NotifyIcon and exits.
    pub fn balloon(&self, title: &str, text: &str) {
        let quote = |s: &str| s.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $n = New-Object System.Windows.Forms.NotifyIcon; \
             $n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; \
             $n.ShowBalloonTip(8000, '{}', '{}', 'Info'); Start-Sleep 9; $n.Dispose()",
            quote(title),
            quote(text)
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn();
    }

    fn show_mode(&self, mode: Mode) {
        self.automatic.set_checked(mode == Mode::Automatic);
        self.disabled.set_checked(mode == Mode::Disabled);
    }

    pub fn show(&self, line: &str, reason: Option<Reason>) {
        self.status.set_text(line);
        let _ = self
            .icon
            .set_tooltip(Some(format!("switchable-ci-runner: {line}")));
        let color = match reason {
            Some(Reason::JobRunning) => Color::Blue,
            Some(Reason::Idle) | Some(Reason::OwnerSaidYes) => Color::Green,
            Some(Reason::InUse) => Color::Yellow,
            Some(Reason::Disabled) | Some(Reason::OnBattery) | None => Color::Gray,
        };
        if let Ok(icon) = dot(color) {
            let _ = self.icon.set_icon(Some(icon));
        }
    }
}

#[derive(Clone, Copy)]
enum Color {
    Green,
    Yellow,
    Blue,
    Gray,
}

/// A filled circle, drawn here so the binary needs no image files.
fn dot(color: Color) -> Result<Icon, String> {
    const SIZE: u32 = 32;
    let (r, g, b) = match color {
        Color::Green => (0x2e, 0xa0, 0x43),
        Color::Yellow => (0xd2, 0x99, 0x22),
        Color::Blue => (0x1f, 0x6f, 0xeb),
        Color::Gray => (0x8b, 0x94, 0x9e),
    };
    let center = (SIZE as f32 - 1.0) / 2.0;
    let radius = SIZE as f32 / 2.0 - 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            // One pixel of soft edge.
            let alpha = (radius + 0.5 - d).clamp(0.0, 1.0);
            rgba.extend_from_slice(&[r, g, b, (alpha * 255.0) as u8]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| format!("tray icon image: {e}"))
}
