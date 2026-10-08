// Presses keys in the frontmost window, for scripts/demo/storyboard.sh's
// DEMO_KEYS on macOS. Needs Accessibility permission for the terminal it runs
// from; scripts/demo/film-macos.sh builds it and checks that first.
//
//   keys trusted [--prompt]   exit 0 when key presses are allowed; with
//                             --prompt, macOS offers to allow them if not
//   keys activate PID         bring that process's app to the front
//   keys chord ctrl+p a ...   press each chord in turn
//   keys type TEXT [MS]       type TEXT, MS milliseconds between characters

import AppKit
import ApplicationServices

let codes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
    "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
    "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31,
    "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
    "return": 36, "tab": 48, "space": 49, "escape": 53,
    "left": 123, "right": 124, "down": 125, "up": 126,
]
let modifiers: [String: CGEventFlags] = [
    "ctrl": .maskControl, "alt": .maskAlternate, "cmd": .maskCommand, "shift": .maskShift,
]
let source = CGEventSource(stateID: .hidSystemState)

func post(_ code: CGKeyCode, flags: CGEventFlags = [], text: String? = nil) {
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else {
            continue
        }
        event.flags = flags
        if let text {
            let units = Array(text.utf16)
            event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
        }
        event.post(tap: .cghidEventTap)
        usleep(15_000)
    }
}

let arguments = Array(CommandLine.arguments.dropFirst())
switch arguments.first {
case "trusted":
    let prompt = arguments.dropFirst().contains("--prompt")
    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: prompt] as CFDictionary
    exit(AXIsProcessTrustedWithOptions(options) ? 0 : 1)
case "activate":
    guard arguments.count == 2, let pid = Int32(arguments[1]),
          let app = NSRunningApplication(processIdentifier: pid) else { exit(2) }
    exit(app.activate(options: [.activateIgnoringOtherApps]) ? 0 : 1)
case "chord":
    for chord in arguments.dropFirst() {
        let parts = chord.lowercased().split(separator: "+").map(String.init)
        guard let key = parts.last, let code = codes[key] else {
            FileHandle.standardError.write("unknown key in \(chord)\n".data(using: .utf8)!)
            exit(2)
        }
        let flags = parts.dropLast().reduce(into: CGEventFlags()) { flags, name in
            flags.insert(modifiers[name] ?? [])
        }
        post(code, flags: flags)
        usleep(140_000)
    }
case "type":
    guard arguments.count >= 2 else { exit(2) }
    let pause = UInt32(arguments.count > 2 ? Int(arguments[2]) ?? 34 : 34) * 1000
    for character in arguments[1] {
        post(0, text: String(character))
        usleep(pause)
    }
default:
    FileHandle.standardError.write("usage: keys trusted [--prompt] | activate PID | chord CHORD... | type TEXT [MS]\n".data(using: .utf8)!)
    exit(2)
}
