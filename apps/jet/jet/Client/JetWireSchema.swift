import CoreFoundation
import Foundation

enum JetWireValidationFailure: Error, Equatable {
    case schemaUnavailable
    case malformedJSON
    case duplicateField
    case nestingLimit
    case collectionLimit
    case schemaMismatch
}

indirect enum JetJSONNode {
    case object([JetJSONField], Range<Int>)
    case array([JetJSONNode], Range<Int>)
    case scalar(Range<Int>)

    var range: Range<Int> {
        switch self {
        case let .object(_, range), let .array(_, range), let .scalar(range):
            range
        }
    }

    func member(_ key: String) -> JetJSONNode? {
        guard case let .object(fields, _) = self else { return nil }
        return fields.last(where: { $0.key == key })?.node
    }

    var elements: [JetJSONNode]? {
        guard case let .array(items, _) = self else { return nil }
        return items
    }
}

struct JetJSONField {
    let key: String
    let node: JetJSONNode
}

struct JetJSONDocument {
    let data: Data
    let root: JetJSONNode
    let value: Any

    func rawJSON(for node: JetJSONNode) throws -> JetRawJSON {
        guard let source = String(
            data: data.subdata(in: node.range),
            encoding: .utf8
        ) else {
            throw JetWireValidationFailure.malformedJSON
        }
        return JetRawJSON(source: source)
    }
}

// JSONSerialization builds immutable schema graphs here. The validator never
// exposes or mutates that graph; tests exercise the same instance concurrently.
struct JetWireSchema: @unchecked Sendable {
    private final class ResourceAnchor: NSObject {}

    private let definitions: [String: Any]

    init(data: Data) throws {
        guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let definitions = root["$defs"] as? [String: Any]
        else {
            throw JetWireValidationFailure.schemaUnavailable
        }
        self.definitions = definitions
    }

    static func bundled() throws -> JetWireSchema {
        let bundle = Bundle(for: ResourceAnchor.self)
        guard let url = bundle.url(forResource: "jet-v1.schema", withExtension: "json") else {
            throw JetWireValidationFailure.schemaUnavailable
        }
        return try JetWireSchema(data: Data(contentsOf: url))
    }

    func validate(_ data: Data, definition: String) throws -> JetJSONDocument {
        guard let schema = definitions[definition] else {
            throw JetWireValidationFailure.schemaUnavailable
        }

        // ASVS 2.1.1, 2.2.1, and 15.1.1: parsing has one documented trust
        // boundary with explicit protocol depth and collection limits.
        var parser = JetBoundedJSONParser(data: data)
        let node = try parser.parse()
        let value: Any
        do {
            value = try JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
        } catch {
            throw JetWireValidationFailure.malformedJSON
        }
        guard uniqueKnownFields(node, schema: schema, value: value) else {
            throw JetWireValidationFailure.duplicateField
        }
        guard matches(schema, value) else {
            throw JetWireValidationFailure.schemaMismatch
        }
        return JetJSONDocument(data: data, root: node, value: value)
    }

    private func uniqueKnownFields(
        _ node: JetJSONNode,
        schema: Any,
        value: Any
    ) -> Bool {
        switch node {
        case let .object(fields, _):
            let object = value as? [String: Any] ?? [:]
            let tags = object.compactMapValues { $0 as? String }
            let known = properties(schema, tags: tags)
            var seen = Set<String>()
            for field in fields {
                if known[field.key] != nil, !seen.insert(field.key).inserted {
                    return false
                }
                if !uniqueKnownFields(
                    field.node,
                    schema: known[field.key] ?? [:],
                    value: object[field.key] ?? NSNull()
                ) {
                    return false
                }
            }
            return true
        case let .array(items, _):
            let values = value as? [Any] ?? []
            let itemSchema = resolve(schema)["items"] ?? [:]
            return zip(items, values).allSatisfy { item, value in
                uniqueKnownFields(item, schema: itemSchema, value: value)
            }
        case .scalar:
            return true
        }
    }

    private func properties(
        _ schema: Any,
        tags: [String: String]
    ) -> [String: Any] {
        let node = resolve(schema)
        var fields = node["properties"] as? [String: Any] ?? [:]
        for (field, constraintValue) in fields {
            guard let tag = tags[field],
                  let constraint = constraintValue as? [String: Any]
            else {
                continue
            }
            if let constant = constraint["const"] as? String, constant != tag {
                return [:]
            }
            if let excluded = constraint["not"] as? [String: Any],
               let values = excluded["enum"] as? [String],
               values.contains(tag) {
                return [:]
            }
        }
        for branch in (node["oneOf"] ?? node["anyOf"]) as? [Any] ?? [] {
            fields.merge(properties(branch, tags: tags)) { _, new in new }
        }
        return fields
    }

    private func resolve(_ schema: Any) -> [String: Any] {
        guard let node = schema as? [String: Any] else { return [:] }
        guard let reference = node["$ref"] as? String,
              let name = reference.split(separator: "/").last,
              let target = definitions[String(name)]
        else {
            return node
        }
        let base = resolve(target)
        var merged = base.merging(node) { _, own in own }
        merged.removeValue(forKey: "$ref")
        merged["properties"] = (
            base["properties"] as? [String: Any] ?? [:]
        ).merging(
            node["properties"] as? [String: Any] ?? [:]
        ) { _, own in own }
        merged["required"] = (base["required"] as? [String] ?? [])
            + (node["required"] as? [String] ?? [])
        return merged
    }

    private func matches(_ schema: Any, _ value: Any) -> Bool {
        if let allowed = schema as? Bool { return allowed }
        guard let node = schema as? [String: Any] else { return false }

        if let reference = node["$ref"] as? String {
            guard let name = reference.split(separator: "/").last,
                  let target = definitions[String(name)],
                  matches(target, value)
            else {
                return false
            }
        }
        if let excluded = node["not"], matches(excluded, value) { return false }
        if let alternatives = node["anyOf"] as? [Any],
           !alternatives.contains(where: { matches($0, value) }) {
            return false
        }
        if let alternatives = node["oneOf"] as? [Any],
           alternatives.filter({ matches($0, value) }).count != 1 {
            return false
        }
        if let constant = node["const"] as? NSObject,
           !constant.isEqual(value) {
            return false
        }
        if let allowed = node["enum"] as? [NSObject],
           !allowed.contains(where: { $0.isEqual(value) }) {
            return false
        }
        if let types = node["type"] as? [String] {
            return types.contains { type in
                var branch = node
                branch["type"] = type
                return matches(branch, value)
            }
        }

        let type = node["type"] as? String
        if type == "null" { return value is NSNull }
        if type == "string" && !(value is String) { return false }
        if let pattern = node["pattern"] as? String,
           let text = value as? String {
            guard let expression = try? NSRegularExpression(pattern: pattern) else {
                return false
            }
            let range = NSRange(location: 0, length: (text as NSString).length)
            if expression.firstMatch(in: text, range: range) == nil { return false }
        }

        let number = value as? NSNumber
        let isBoolean = number.map { CFGetTypeID($0) == CFBooleanGetTypeID() } ?? false
        if type == "boolean" && !isBoolean { return false }
        if type == "integer" || type == "number" {
            guard let number, !isBoolean else { return false }
            let numericValue = number.doubleValue
            if type == "integer" && numericValue.rounded() != numericValue { return false }
            if let minimum = node["minimum"] as? NSNumber,
               numericValue < minimum.doubleValue {
                return false
            }
            if let maximum = node["maximum"] as? NSNumber,
               numericValue > maximum.doubleValue {
                return false
            }
        }

        if type == "array" {
            guard let items = value as? [Any] else { return false }
            if let itemSchema = node["items"] {
                return items.allSatisfy { matches(itemSchema, $0) }
            }
        }

        if type == "object" {
            guard let object = value as? [String: Any] else { return false }
            if let required = node["required"] as? [String],
               !required.allSatisfy({ object[$0] != nil }) {
                return false
            }
            let declared = node["properties"] as? [String: Any] ?? [:]
            if node["additionalProperties"] as? Bool == false,
               object.keys.contains(where: { declared[$0] == nil }) {
                return false
            }
            return declared.allSatisfy { key, fieldSchema in
                object[key].map { matches(fieldSchema, $0) } ?? true
            }
        }

        return true
    }
}

private struct JetBoundedJSONParser {
    private static let maximumDepth = 64
    private static let maximumDirectItems = 4_096
    private static let maximumTotalItems = 8_192

    private let data: Data
    private let bytes: [UInt8]
    private var index = 0
    private var totalItems = 0

    init(data: Data) {
        self.data = data
        bytes = Array(data)
    }

    mutating func parse() throws -> JetJSONNode {
        skipWhitespace()
        let root = try parseValue(depth: 1)
        skipWhitespace()
        guard index == bytes.count else {
            throw JetWireValidationFailure.malformedJSON
        }
        return root
    }

    private mutating func parseValue(depth: Int) throws -> JetJSONNode {
        guard depth <= Self.maximumDepth, index < bytes.count else {
            throw depth > Self.maximumDepth
                ? JetWireValidationFailure.nestingLimit
                : JetWireValidationFailure.malformedJSON
        }
        switch bytes[index] {
        case 0x7b: return try parseObject(depth: depth)
        case 0x5b: return try parseArray(depth: depth)
        case 0x22:
            let range = try parseStringRange()
            return .scalar(range)
        case 0x74: return try parseLiteral(Array("true".utf8))
        case 0x66: return try parseLiteral(Array("false".utf8))
        case 0x6e: return try parseLiteral(Array("null".utf8))
        case 0x2d, 0x30...0x39: return try parseNumber()
        default: throw JetWireValidationFailure.malformedJSON
        }
    }

    private mutating func parseObject(depth: Int) throws -> JetJSONNode {
        let start = index
        index += 1
        skipWhitespace()
        var fields: [JetJSONField] = []
        var directItems = 0
        if consume(0x7d) { return .object(fields, start..<index) }

        while true {
            let keyRange = try parseStringRange()
            let key = try decodedString(in: keyRange)
            skipWhitespace()
            guard consume(0x3a) else {
                throw JetWireValidationFailure.malformedJSON
            }
            skipWhitespace()
            fields.append(JetJSONField(
                key: key,
                node: try parseValue(depth: depth + 1)
            ))
            try countItem(&directItems)
            skipWhitespace()
            if consume(0x7d) { break }
            guard consume(0x2c) else {
                throw JetWireValidationFailure.malformedJSON
            }
            skipWhitespace()
        }
        return .object(fields, start..<index)
    }

    private mutating func parseArray(depth: Int) throws -> JetJSONNode {
        let start = index
        index += 1
        skipWhitespace()
        var items: [JetJSONNode] = []
        var directItems = 0
        if consume(0x5d) { return .array(items, start..<index) }

        while true {
            items.append(try parseValue(depth: depth + 1))
            try countItem(&directItems)
            skipWhitespace()
            if consume(0x5d) { break }
            guard consume(0x2c) else {
                throw JetWireValidationFailure.malformedJSON
            }
            skipWhitespace()
        }
        return .array(items, start..<index)
    }

    private mutating func parseStringRange() throws -> Range<Int> {
        let start = index
        guard consume(0x22) else { throw JetWireValidationFailure.malformedJSON }
        var escaped = false
        while index < bytes.count {
            let byte = bytes[index]
            index += 1
            if escaped {
                if byte == 0x75 {
                    guard index + 4 <= bytes.count,
                          bytes[index..<(index + 4)].allSatisfy(isHexDigit)
                    else {
                        throw JetWireValidationFailure.malformedJSON
                    }
                    index += 4
                } else if ![0x22, 0x5c, 0x2f, 0x62, 0x66, 0x6e, 0x72, 0x74].contains(byte) {
                    throw JetWireValidationFailure.malformedJSON
                }
                escaped = false
            } else if byte == 0x5c {
                escaped = true
            } else if byte == 0x22 {
                return start..<index
            } else if byte < 0x20 {
                throw JetWireValidationFailure.malformedJSON
            }
        }
        throw JetWireValidationFailure.malformedJSON
    }

    private mutating func parseLiteral(_ literal: [UInt8]) throws -> JetJSONNode {
        let start = index
        guard index + literal.count <= bytes.count,
              Array(bytes[index..<(index + literal.count)]) == literal
        else {
            throw JetWireValidationFailure.malformedJSON
        }
        index += literal.count
        return .scalar(start..<index)
    }

    private mutating func parseNumber() throws -> JetJSONNode {
        let start = index
        if consume(0x2d), index == bytes.count {
            throw JetWireValidationFailure.malformedJSON
        }
        if consume(0x30) {
            if index < bytes.count, (0x30...0x39).contains(bytes[index]) {
                throw JetWireValidationFailure.malformedJSON
            }
        } else {
            guard consumeDigit(allowZero: false) else {
                throw JetWireValidationFailure.malformedJSON
            }
            while consumeDigit(allowZero: true) {}
        }
        if consume(0x2e) {
            guard consumeDigit(allowZero: true) else {
                throw JetWireValidationFailure.malformedJSON
            }
            while consumeDigit(allowZero: true) {}
        }
        if index < bytes.count, bytes[index] == 0x65 || bytes[index] == 0x45 {
            index += 1
            if index < bytes.count, bytes[index] == 0x2b || bytes[index] == 0x2d {
                index += 1
            }
            guard consumeDigit(allowZero: true) else {
                throw JetWireValidationFailure.malformedJSON
            }
            while consumeDigit(allowZero: true) {}
        }
        return .scalar(start..<index)
    }

    private mutating func countItem(_ directItems: inout Int) throws {
        directItems += 1
        totalItems += 1
        guard directItems <= Self.maximumDirectItems,
              totalItems <= Self.maximumTotalItems
        else {
            throw JetWireValidationFailure.collectionLimit
        }
    }

    private func decodedString(in range: Range<Int>) throws -> String {
        let fragment = data.subdata(in: range)
        guard let value = try JSONSerialization.jsonObject(
            with: fragment,
            options: [.fragmentsAllowed]
        ) as? String else {
            throw JetWireValidationFailure.malformedJSON
        }
        return value
    }

    private mutating func skipWhitespace() {
        while index < bytes.count,
              [0x20, 0x09, 0x0a, 0x0d].contains(bytes[index]) {
            index += 1
        }
    }

    private mutating func consume(_ byte: UInt8) -> Bool {
        guard index < bytes.count, bytes[index] == byte else { return false }
        index += 1
        return true
    }

    private mutating func consumeDigit(allowZero: Bool) -> Bool {
        guard index < bytes.count else { return false }
        let range: ClosedRange<UInt8> = allowZero ? 0x30...0x39 : 0x31...0x39
        guard range.contains(bytes[index]) else { return false }
        index += 1
        return true
    }

    private func isHexDigit(_ byte: UInt8) -> Bool {
        (0x30...0x39).contains(byte)
            || (0x41...0x46).contains(byte)
            || (0x61...0x66).contains(byte)
    }
}
