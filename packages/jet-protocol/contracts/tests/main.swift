// Test-only interpreter for the emitted schema; original payloads remain authoritative.
import Foundation
import CoreFoundation

let root = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))) as! [String: Any]
let definitions = root["$defs"] as! [String: Any]
func resolve(_ schema: Any) -> [String: Any] {
    guard let node = schema as? [String: Any] else { return [:] }
    if let ref = node["$ref"] as? String { return resolve(definitions[String(ref.split(separator: "/").last!)]!) }
    return node
}
func properties(_ schema: Any, kind: String? = nil) -> [String: Any] {
    let node = resolve(schema)
    var fields = node["properties"] as? [String: Any] ?? [:]
    if let kind, let tag = fields["kind"] as? [String: Any] {
        if let constant = tag["const"] as? String, constant != kind { return [:] }
        if let not = tag["not"] as? [String: Any], let excluded = not["enum"] as? [String], excluded.contains(kind) { return [:] }
    }
    for branch in (node["oneOf"] ?? node["anyOf"]) as? [Any] ?? [] {
        fields.merge(properties(branch, kind: kind)) { _, new in new }
    }
    return fields
}
func uniqueFields(_ source: String, _ schema: Any) -> Bool {
    // Raw native content remains uninterpreted; only known wire fields are checked.
    let pattern = #""(?:\\.|[^"\\])*"|[{}\[\],:]|[^{}\[\],:\s]+"#
    let regex = try! NSRegularExpression(pattern: pattern)
    let ns = source as NSString
    let tokens = regex.matches(in: source, range: NSRange(location: 0, length: ns.length)).map { ns.substring(with: $0.range) }
    var index = 0, valid = true
    func objectKind() -> String? {
        var depth = 0, kind: String?
        for cursor in index..<tokens.count {
            let token = tokens[cursor]
            if token == "{" || token == "[" { depth += 1 }
            else if token == "}" || token == "]" { if depth == 0 { break }; depth -= 1 }
            else if depth == 0 && token.hasPrefix("\"") && cursor + 2 < tokens.count && tokens[cursor + 1] == ":" && tokens[cursor + 2].hasPrefix("\"") {
                let key = try! JSONSerialization.jsonObject(with: Data(token.utf8), options: [.fragmentsAllowed]) as! String
                if key == "kind" { kind = try! JSONSerialization.jsonObject(with: Data(tokens[cursor + 2].utf8), options: [.fragmentsAllowed]) as? String }
            }
        }
        return kind
    }
    func visit(_ s: Any) {
        guard index < tokens.count else { valid = false; return }
        let token = tokens[index]; index += 1
        if token == "{" {
            let fields = properties(s, kind: objectKind())
            var seen = Set<String>()
            while index < tokens.count && tokens[index] != "}" {
                let key = try! JSONSerialization.jsonObject(with: Data(tokens[index].utf8), options: [.fragmentsAllowed]) as! String
                index += 1
                if fields[key] != nil && seen.contains(key) { valid = false }
                seen.insert(key)
                guard index < tokens.count && tokens[index] == ":" else { valid = false; return }
                index += 1
                visit(fields[key] ?? [:])
                if index < tokens.count && tokens[index] == "," { index += 1 }
            }
            if index == tokens.count || tokens[index] != "}" { valid = false }; index += 1
        } else if token == "[" {
            while index < tokens.count && tokens[index] != "]" {
                visit(resolve(s)["items"] ?? [:])
                if index < tokens.count && tokens[index] == "," { index += 1 }
            }
            if index == tokens.count || tokens[index] != "]" { valid = false }; index += 1
        }
    }
    visit(schema)
    return valid && index == tokens.count
}
func matches(_ schema: Any, _ value: Any) -> Bool {
    if let allowed = schema as? Bool { return allowed }
    let s = schema as! [String: Any]
    if let ref = s["$ref"] as? String { return matches(definitions[String(ref.split(separator: "/").last!)]!, value) }
    if let not = s["not"], matches(not, value) { return false }
    if let any = s["anyOf"] as? [Any], !any.contains(where: { matches($0, value) }) { return false }
    if let one = s["oneOf"] as? [Any], one.filter({ matches($0, value) }).count != 1 { return false }
    if let constant = s["const"] as? NSObject, !constant.isEqual(value) { return false }
    if let values = s["enum"] as? [NSObject], !values.contains(where: { $0.isEqual(value) }) { return false }
    if let types = s["type"] as? [String] { return types.contains { var branch = s; branch["type"] = $0; return matches(branch, value) } }
    let type = s["type"] as? String
    if type == "null" { return value is NSNull }
    if type == "string" && !(value is String) { return false }
    let number = value as? NSNumber
    let boolean = number.map { CFGetTypeID($0) == CFBooleanGetTypeID() } ?? false
    if type == "boolean" && !boolean { return false }
    if type == "integer" || type == "number" {
        guard let number, !boolean else { return false }
        let n = number.doubleValue
        if type == "integer" && n.rounded() != n { return false }
        if let minimum = s["minimum"] as? NSNumber, n < minimum.doubleValue { return false }
        if let maximum = s["maximum"] as? NSNumber, n > maximum.doubleValue { return false }
    }
    if type == "array" {
        guard let items = value as? [Any] else { return false }
        return items.allSatisfy { matches(s["items"]!, $0) }
    }
    if type == "object" {
        guard let object = value as? [String: Any] else { return false }
        if let required = s["required"] as? [String], !required.allSatisfy({ object[$0] != nil }) { return false }
        return (s["properties"] as? [String: Any] ?? [:]).allSatisfy { key, part in object[key].map { matches(part, $0) } ?? true }
    }
    return true
}

let fixtures = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2]))) as! [[String: Any]]
for fixture in fixtures {
    let source = fixture["payload"] as! String
    let schema = definitions[fixture["schema"] as! String]!
    let unique = uniqueFields(source, schema)
    let view = try JSONSerialization.jsonObject(with: Data(source.utf8))
    precondition((unique && matches(schema, view)) == (fixture["valid"] as! Bool), source)
}
print("Swift: \(fixtures.count) shared Craft fixtures passed")
