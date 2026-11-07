// Prints a process's accessibility tree through the AX API itself, as VoiceOver reads it: one line
// per element, `role [title] value @x,y`, indented by depth. System Events (scripts/ax.js) resolves
// elements by their index among siblings and can hand back another element's text and frame when a
// window repeats one structure (a list of job cards), so checks of such a list read this instead.
//
//   swift app/scripts/axdump.swift <pid>
import ApplicationServices
import Foundation

guard CommandLine.arguments.count == 2, let pid = pid_t(CommandLine.arguments[1]) else {
    FileHandle.standardError.write("usage: axdump.swift <pid>\n".data(using: .utf8)!)
    exit(2)
}

func attr(_ e: AXUIElement, _ a: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, a as CFString, &v) == .success ? v : nil
}

func origin(_ e: AXUIElement) -> String {
    var p = CGPoint.zero
    if let v = attr(e, kAXPositionAttribute) { AXValueGetValue(v as! AXValue, .cgPoint, &p) }
    return "\(Int(p.x)),\(Int(p.y))"
}

func walk(_ e: AXUIElement, _ depth: Int) {
    let role = attr(e, kAXRoleAttribute) as? String ?? "?"
    let value = attr(e, kAXValueAttribute).map { "\($0)" } ?? ""
    let title = attr(e, kAXTitleAttribute) as? String ?? attr(e, kAXDescriptionAttribute) as? String ?? ""
    print(String(repeating: "  ", count: depth) + "\(role) [\(title)] \(value) @\(origin(e))")
    for child in attr(e, kAXChildrenAttribute) as? [AXUIElement] ?? [] { walk(child, depth + 1) }
}

for window in attr(AXUIElementCreateApplication(pid), kAXWindowsAttribute) as? [AXUIElement] ?? [] {
    walk(window, 0)
}
