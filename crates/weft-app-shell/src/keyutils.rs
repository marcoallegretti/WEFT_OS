#![cfg(feature = "servo-embed")]

use servo::{Code, Key, KeyState, KeyboardEvent, Location, Modifiers};
use winit::event::ElementState;
use winit::keyboard::{Key as WinitKey, KeyLocation, ModifiersState, NamedKey, PhysicalKey};

pub fn keyboard_event_from_winit(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> KeyboardEvent {
    let state = match event.state {
        ElementState::Pressed => KeyState::Down,
        ElementState::Released => KeyState::Up,
    };

    let key = match &event.logical_key {
        WinitKey::Named(n) => Key::Named(named_key(*n)),
        WinitKey::Character(c) => Key::Character(c.to_string().into()),
        WinitKey::Unidentified(_) => Key::Unidentified,
        WinitKey::Dead(c) => Key::Dead(*c),
    };

    let code = match event.physical_key {
        PhysicalKey::Code(c) => winit_code_to_code(c),
        PhysicalKey::Unidentified(_) => Code::Unidentified,
    };

    let location = match event.location {
        KeyLocation::Standard => Location::Standard,
        KeyLocation::Left => Location::Left,
        KeyLocation::Right => Location::Right,
        KeyLocation::Numpad => Location::Numpad,
    };

    let mut mods = Modifiers::empty();
    if modifiers.shift_key() {
        mods |= Modifiers::SHIFT;
    }
    if modifiers.control_key() {
        mods |= Modifiers::CONTROL;
    }
    if modifiers.alt_key() {
        mods |= Modifiers::ALT;
    }
    if modifiers.super_key() {
        mods |= Modifiers::META;
    }

    KeyboardEvent {
        state,
        key,
        code,
        location,
        modifiers: mods,
        repeat: event.repeat,
        is_composing: false,
    }
}

fn named_key(n: NamedKey) -> servo::NamedKey {
    use servo::NamedKey as S;
    match n {
        NamedKey::Alt => S::Alt,
        NamedKey::AltGraph => S::AltGraph,
        NamedKey::CapsLock => S::CapsLock,
        NamedKey::Control => S::Control,
        NamedKey::Fn => S::Fn,
        NamedKey::FnLock => S::FnLock,
        NamedKey::Meta => S::Meta,
        NamedKey::NumLock => S::NumLock,
        NamedKey::ScrollLock => S::ScrollLock,
        NamedKey::Shift => S::Shift,
        NamedKey::Symbol => S::Symbol,
        NamedKey::SymbolLock => S::SymbolLock,
        NamedKey::Enter => S::Enter,
        NamedKey::Tab => S::Tab,
        NamedKey::Space => S::Space,
        NamedKey::ArrowDown => S::ArrowDown,
        NamedKey::ArrowLeft => S::ArrowLeft,
        NamedKey::ArrowRight => S::ArrowRight,
        NamedKey::ArrowUp => S::ArrowUp,
        NamedKey::End => S::End,
        NamedKey::Home => S::Home,
        NamedKey::PageDown => S::PageDown,
        NamedKey::PageUp => S::PageUp,
        NamedKey::Backspace => S::Backspace,
        NamedKey::Clear => S::Clear,
        NamedKey::Copy => S::Copy,
        NamedKey::CrSel => S::CrSel,
        NamedKey::Cut => S::Cut,
        NamedKey::Delete => S::Delete,
        NamedKey::EraseEof => S::EraseEof,
        NamedKey::ExSel => S::ExSel,
        NamedKey::Insert => S::Insert,
        NamedKey::Paste => S::Paste,
        NamedKey::Redo => S::Redo,
        NamedKey::Undo => S::Undo,
        NamedKey::Escape => S::Escape,
        NamedKey::F1 => S::F1,
        NamedKey::F2 => S::F2,
        NamedKey::F3 => S::F3,
        NamedKey::F4 => S::F4,
        NamedKey::F5 => S::F5,
        NamedKey::F6 => S::F6,
        NamedKey::F7 => S::F7,
        NamedKey::F8 => S::F8,
        NamedKey::F9 => S::F9,
        NamedKey::F10 => S::F10,
        NamedKey::F11 => S::F11,
        NamedKey::F12 => S::F12,
        _ => S::Unidentified,
    }
}

fn winit_code_to_code(c: winit::keyboard::KeyCode) -> Code {
    use winit::keyboard::KeyCode as W;
    match c {
        W::Backquote => Code::Backquote,
        W::Backslash => Code::Backslash,
        W::BracketLeft => Code::BracketLeft,
        W::BracketRight => Code::BracketRight,
        W::Comma => Code::Comma,
        W::Digit0 => Code::Digit0,
        W::Digit1 => Code::Digit1,
        W::Digit2 => Code::Digit2,
        W::Digit3 => Code::Digit3,
        W::Digit4 => Code::Digit4,
        W::Digit5 => Code::Digit5,
        W::Digit6 => Code::Digit6,
        W::Digit7 => Code::Digit7,
        W::Digit8 => Code::Digit8,
        W::Digit9 => Code::Digit9,
        W::Equal => Code::Equal,
        W::KeyA => Code::KeyA,
        W::KeyB => Code::KeyB,
        W::KeyC => Code::KeyC,
        W::KeyD => Code::KeyD,
        W::KeyE => Code::KeyE,
        W::KeyF => Code::KeyF,
        W::KeyG => Code::KeyG,
        W::KeyH => Code::KeyH,
        W::KeyI => Code::KeyI,
        W::KeyJ => Code::KeyJ,
        W::KeyK => Code::KeyK,
        W::KeyL => Code::KeyL,
        W::KeyM => Code::KeyM,
        W::KeyN => Code::KeyN,
        W::KeyO => Code::KeyO,
        W::KeyP => Code::KeyP,
        W::KeyQ => Code::KeyQ,
        W::KeyR => Code::KeyR,
        W::KeyS => Code::KeyS,
        W::KeyT => Code::KeyT,
        W::KeyU => Code::KeyU,
        W::KeyV => Code::KeyV,
        W::KeyW => Code::KeyW,
        W::KeyX => Code::KeyX,
        W::KeyY => Code::KeyY,
        W::KeyZ => Code::KeyZ,
        W::Minus => Code::Minus,
        W::Period => Code::Period,
        W::Quote => Code::Quote,
        W::Semicolon => Code::Semicolon,
        W::Slash => Code::Slash,
        W::Backspace => Code::Backspace,
        W::CapsLock => Code::CapsLock,
        W::Enter => Code::Enter,
        W::Space => Code::Space,
        W::Tab => Code::Tab,
        W::Delete => Code::Delete,
        W::End => Code::End,
        W::Home => Code::Home,
        W::Insert => Code::Insert,
        W::PageDown => Code::PageDown,
        W::PageUp => Code::PageUp,
        W::ArrowDown => Code::ArrowDown,
        W::ArrowLeft => Code::ArrowLeft,
        W::ArrowRight => Code::ArrowRight,
        W::ArrowUp => Code::ArrowUp,
        W::NumLock => Code::NumLock,
        W::Escape => Code::Escape,
        W::F1 => Code::F1,
        W::F2 => Code::F2,
        W::F3 => Code::F3,
        W::F4 => Code::F4,
        W::F5 => Code::F5,
        W::F6 => Code::F6,
        W::F7 => Code::F7,
        W::F8 => Code::F8,
        W::F9 => Code::F9,
        W::F10 => Code::F10,
        W::F11 => Code::F11,
        W::F12 => Code::F12,
        W::ShiftLeft => Code::ShiftLeft,
        W::ShiftRight => Code::ShiftRight,
        W::ControlLeft => Code::ControlLeft,
        W::ControlRight => Code::ControlRight,
        W::AltLeft => Code::AltLeft,
        W::AltRight => Code::AltRight,
        W::SuperLeft => Code::MetaLeft,
        W::SuperRight => Code::MetaRight,
        _ => Code::Unidentified,
    }
}
