import Foundation
import Observation
import SwiftUI

// MARK: - Copy

/// How a daily message reads: "Every day at 9:00 AM (Berlin)".
enum ScheduleCopy {
    /// The zone's city: its identifier's last component, with spaces for underscores.
    static func city(_ timeZoneID: String) -> String {
        (timeZoneID.split(separator: "/").last.map(String.init) ?? timeZoneID)
            .replacingOccurrences(of: "_", with: " ")
    }

    /// "Berlin · Central European Time".
    static func zoneLabel(_ timeZoneID: String, locale: Locale = .autoupdatingCurrent) -> String {
        let city = city(timeZoneID)
        guard let name = TimeZone(identifier: timeZoneID)?.localizedName(for: .generic, locale: locale),
              name != city
        else { return city }
        return String(localized: "\(city) · \(name)")
    }

    /// "HH:MM:SS" as the locale writes a time of day, such as "9:00 AM".
    static func time(localTime: String, locale: Locale = .autoupdatingCurrent) -> String {
        let parts = localTime.split(separator: ":").compactMap { Int($0) }
        guard parts.count >= 2 else { return localTime }
        return time(hour: parts[0], minute: parts[1], locale: locale)
    }

    static func time(hour: Int, minute: Int, locale: Locale = .autoupdatingCurrent) -> String {
        let utc = TimeZone(identifier: "UTC") ?? .gmt
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = utc
        let date = calendar.date(from: DateComponents(year: 2026, month: 1, day: 15, hour: hour, minute: minute)) ?? .now
        return date.formatted(Date.FormatStyle(locale: locale, calendar: calendar, timeZone: utc).hour().minute())
    }

    static func summary(localTime: String, timeZone: String, locale: Locale = .autoupdatingCurrent) -> String {
        let time = time(localTime: localTime, locale: locale)
        return String(localized: "Every day at \(time) (\(city(timeZone)))")
    }

    /// The notice after saving: "Repeats daily at 9:00 AM (Berlin)."
    static func savedNotice(localTime: String, timeZone: String, locale: Locale = .autoupdatingCurrent) -> String {
        let time = time(localTime: localTime, locale: locale)
        return String(localized: "Repeats daily at \(time) (\(city(timeZone))).")
    }

    /// "Next: Sep 29 at 9:00 AM", in the schedule's own zone.
    static func next(
        unixMilliseconds: Int64,
        timeZone: String,
        now: Date = .now,
        locale: Locale = .autoupdatingCurrent
    ) -> String {
        let zone = TimeZone(identifier: timeZone) ?? .autoupdatingCurrent
        let date = Date(timeIntervalSince1970: TimeInterval(unixMilliseconds) / 1_000)
        let day = LibraryCopy.shortDate(date, now: now, locale: locale, timeZone: zone)
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = zone
        let time = date.formatted(Date.FormatStyle(locale: locale, calendar: calendar, timeZone: zone).hour().minute())
        return String(localized: "Next: \(day) at \(time)")
    }
}

// MARK: - Model

/// Repeat Daily… for one task: its daily messages and a new one (design §6.11).
@MainActor
@Observable
final class RepeatDailyModel {
    enum Phase: Equatable, Sendable {
        case loading
        case loaded
        case unavailable
    }

    /// A create whose outcome is unknown, kept with its Command ID and exact body.
    struct CreateRequest: Equatable, Sendable {
        let commandID: UUID
        let timeZone: String
        let localTime: String
        let prompt: String
        let knownScheduleIDs: Set<UUID>
    }

    struct StopRequest: Equatable, Sendable {
        let scheduleID: UUID
        let commandID: UUID
    }

    static let maximumBytes = 8_192
    static let maximumSchedules = 32

    let ref: ConversationRef
    let computer: String
    let isLocal: Bool
    @ObservationIgnored private let makeAccess: JetLibraryAccessProvider
    @ObservationIgnored private var isSeeded = false

    var message = ""
    var hour = 9
    var minute = 0
    var timeZoneID = TimeZone.current.identifier
    private(set) var phase: Phase = .loading
    private(set) var schedules: [JetScheduledTask] = []
    private(set) var isSaving = false
    private(set) var stoppingID: UUID?
    /// The schedule waiting for "Stop repeating this message?".
    var pendingStop: JetScheduledTask?
    var issue: LibraryIssue?
    private(set) var uncertainCreate: CreateRequest?
    private(set) var uncertainStop: StopRequest?

    init(
        ref: ConversationRef,
        makeAccess: @escaping JetLibraryAccessProvider,
        computer: String = String(localized: "This Mac"),
        isLocal: Bool = true
    ) {
        self.ref = ref
        self.makeAccess = makeAccess
        self.computer = computer
        self.isLocal = isLocal
    }

    /// The wall-clock time the daemon expects, "HH:MM:00".
    var localTime: String { String(format: "%02d:%02d:00", hour, minute) }

    var messageBytes: Int { message.utf8.count }

    /// Past 80% of the limit the sheet shows the byte count.
    var showsByteCount: Bool { messageBytes > Self.maximumBytes * 8 / 10 }

    var isDraftEmpty: Bool { message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var canSave: Bool {
        !isDraftEmpty
            && messageBytes <= Self.maximumBytes
            && schedules.count < Self.maximumSchedules
            && phase == .loaded
            && !isSaving
    }

    /// The time as a date in `zone`, for the time picker. Only the hour and minute matter.
    func time(in zone: TimeZone) -> Date {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = zone
        return calendar.date(from: DateComponents(year: 2026, month: 1, day: 15, hour: hour, minute: minute)) ?? .now
    }

    func setTime(_ date: Date, in zone: TimeZone) {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = zone
        let parts = calendar.dateComponents([.hour, .minute], from: date)
        hour = parts.hour ?? hour
        minute = parts.minute ?? minute
    }

    // MARK: Loading

    func load() async {
        guard !isSeeded else { return }
        if schedules.isEmpty { phase = .loading }
        do {
            let access = try await makeAccess(ref.planeRegistryID)
            schedules = try await access.scheduledTasks(conversationID: ref.conversationID).tasks
            phase = .loaded
            if uncertainCreate == nil, uncertainStop == nil { issue = nil }
        } catch {
            guard !(error is CancellationError) else { return }
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
            if schedules.isEmpty { phase = .unavailable }
        }
    }

    // MARK: Saving

    /// Repeat Daily: one create Command. Returns the notice text once it exists.
    func save() async -> String? {
        guard canSave else { return nil }
        let request = CreateRequest(
            commandID: UUID(),
            timeZone: timeZoneID,
            localTime: localTime,
            prompt: message,
            knownScheduleIDs: Set(schedules.map(\.id))
        )
        return await create(request)
    }

    /// Send Same Request Again for an uncertain create or stop.
    func sendSameRequestAgain() async -> String? {
        if let request = uncertainCreate { return await create(request) }
        if let request = uncertainStop { await stop(request) }
        return nil
    }

    private func create(_ request: CreateRequest) async -> String? {
        guard !isSaving else { return nil }
        isSaving = true
        issue = nil
        defer { isSaving = false }
        let done = ScheduleCopy.savedNotice(localTime: request.localTime, timeZone: request.timeZone)
        do {
            let access = try await makeAccess(ref.planeRegistryID)
            do {
                _ = try await access.createSchedule(
                    conversationID: ref.conversationID,
                    timeZone: request.timeZone,
                    localTime: request.localTime,
                    prompt: request.prompt,
                    commandID: request.commandID
                )
            } catch let error where LibraryIssue.isOutcomeUnknown(error) {
                // One confirming Query: a new matching schedule means it was created.
                let latest = try? await access.scheduledTasks(conversationID: ref.conversationID).tasks
                if let latest { schedules = latest }
                let created = latest?.contains {
                    !request.knownScheduleIDs.contains($0.id)
                        && $0.timeZone == request.timeZone
                        && $0.localTime == request.localTime
                        && $0.prompt == request.prompt
                } ?? false
                guard created else {
                    uncertainCreate = request
                    issue = LibraryIssue(
                        kind: .warning,
                        text: String(localized: "Jet couldn't confirm the new daily message."),
                        action: .sendSameRequestAgain,
                        code: LibraryIssue.code(of: error)
                    )
                    return nil
                }
            }
            uncertainCreate = nil
            message = ""
            if let latest = try? await access.scheduledTasks(conversationID: ref.conversationID).tasks {
                schedules = latest
            }
            return done
        } catch {
            // A definite refusal; the next Repeat Daily is a new Command.
            uncertainCreate = nil
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
            return nil
        }
    }

    /// Stop Repeating: one cancel Command for the confirmed schedule.
    func confirmStop() async {
        guard let schedule = pendingStop else { return }
        pendingStop = nil
        let commandID = uncertainStop?.scheduleID == schedule.id ? uncertainStop?.commandID : nil
        await stop(StopRequest(scheduleID: schedule.id, commandID: commandID ?? UUID()))
    }

    private func stop(_ request: StopRequest) async {
        guard stoppingID == nil else { return }
        stoppingID = request.scheduleID
        issue = nil
        defer { stoppingID = nil }
        do {
            let access = try await makeAccess(ref.planeRegistryID)
            do {
                try await access.cancelSchedule(request.scheduleID, commandID: request.commandID)
            } catch let error where LibraryIssue.isOutcomeUnknown(error) {
                // One confirming Query: the schedule is gone only if it stopped.
                let latest = try? await access.scheduledTasks(conversationID: ref.conversationID).tasks
                if let latest { schedules = latest }
                guard let latest, !latest.contains(where: { $0.id == request.scheduleID }) else {
                    uncertainStop = request
                    issue = LibraryIssue(
                        kind: .warning,
                        text: String(localized: "Jet couldn't confirm that the message stopped repeating."),
                        action: .sendSameRequestAgain,
                        code: LibraryIssue.code(of: error)
                    )
                    return
                }
            }
            uncertainStop = nil
            schedules.removeAll { $0.id == request.scheduleID }
            if let latest = try? await access.scheduledTasks(conversationID: ref.conversationID).tasks {
                schedules = latest
            }
        } catch {
            if uncertainStop?.scheduleID == request.scheduleID { uncertainStop = nil }
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
        }
    }

#if DEBUG
    /// A fixed state for previews and screenshots; `load` then does nothing.
    func seedForPreview(schedules: [JetScheduledTask], message: String = "", timeZoneID: String? = nil) {
        isSeeded = true
        self.schedules = schedules
        self.message = message
        if let timeZoneID { self.timeZoneID = timeZoneID }
        phase = .loaded
    }
#endif
}

// MARK: - Sheet

/// Repeat Daily…: Jet sends a message to the task's assistant every day.
struct RepeatDailySheet: View {
    let session: DesktopSession
    let ref: ConversationRef
    @State private var model: RepeatDailyModel
    @State private var showsTimeZones = false

    init(session: DesktopSession, ref: ConversationRef, model: RepeatDailyModel? = nil) {
        self.session = session
        self.ref = ref
        _model = State(initialValue: model ?? RepeatDailyModel(
            ref: ref,
            makeAccess: session.libraryAccessProvider,
            computer: session.planeDisplayName(ref.planeRegistryID),
            isLocal: session.isLocalPlane(ref.planeRegistryID)
        ))
    }

    private var taskTitle: String {
        let title = session.conversations.first { $0.id == ref.conversationID }?.title
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return title.isEmpty ? LibraryCopy.untitledTask : title
    }

    private var assistant: String {
        session.assistantName(for: ref.conversationID) ?? String(localized: "the assistant")
    }

    private var zone: TimeZone { TimeZone(identifier: model.timeZoneID) ?? .autoupdatingCurrent }

    /// The task hasn't started yet, so the first message waits for it.
    private var hasNoRuns: Bool {
        if session.selectedConversationID == ref.conversationID,
           let snapshot = session.conversationSnapshot,
           snapshot.conversation.id == ref.conversationID
        {
            return snapshot.runs.isEmpty
        }
        return session.statusStore.facts[ref.conversationID]?.hasRuns == false
    }

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: JetDesign.gap) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Repeat Daily")
                    .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
                Text("Jet sends this message to \(assistant) in “\(taskTitle)” every day.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if let issue = model.issue {
                LibraryNoticeRow(issue: issue, perform: perform)
            }

            if model.phase == .loading {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("Loading…").foregroundStyle(.secondary)
                }
            } else if !model.schedules.isEmpty {
                repeats
            }

            newMessage

            HStack(spacing: JetDesign.smallGap) {
                Spacer()
                Button(model.isDraftEmpty ? "Done" : "Cancel") { session.dismissSheet() }
                    .keyboardShortcut(.cancelAction)
                Button("Repeat Daily", action: save)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!model.canSave)
                    .accessibilityIdentifier("repeat-daily-save")
            }
        }
        .padding(JetDesign.sectionGap)
        .frame(width: 520, alignment: .leading)
        .accessibilityIdentifier("repeat-daily-sheet")
        .task { await model.load() }
        .confirmationDialog(
            "Stop repeating this message?",
            isPresented: Binding(
                get: { model.pendingStop != nil },
                set: { if !$0 { model.pendingStop = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Stop Repeating", role: .destructive) { Task { await model.confirmStop() } }
            Button("Cancel", role: .cancel) { model.pendingStop = nil }
        } message: {
            Text("It won't be sent again. If it's waiting to send, it's removed. A reply in progress continues.")
        }
    }

    // MARK: Repeats

    private var repeats: some View {
        VStack(alignment: .leading, spacing: JetDesign.smallGap) {
            Text("Repeats")
                .font(.headline)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(model.schedules.enumerated()), id: \.element.id) { index, schedule in
                        if index > 0 { Divider() }
                        scheduleRow(schedule)
                    }
                }
            }
            .scrollBounceBehavior(.basedOnSize)
            .frame(maxHeight: 220)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func scheduleRow(_ schedule: JetScheduledTask) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: JetDesign.smallGap) {
            VStack(alignment: .leading, spacing: 2) {
                Text(ScheduleCopy.summary(localTime: schedule.localTime, timeZone: schedule.timeZone))
                    .fontWeight(.medium)
                Text(schedule.prompt)
                    .lineLimit(2)
                    .foregroundStyle(.secondary)
                Text(ScheduleCopy.next(
                    unixMilliseconds: schedule.nextDueAtUnixMilliseconds,
                    timeZone: schedule.timeZone,
                    now: session.libraryNow
                ))
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: JetDesign.smallGap)
            if model.stoppingID == schedule.id {
                ProgressView().controlSize(.small)
            } else {
                Button("Stop Repeating…") { model.pendingStop = schedule }
                    .buttonStyle(.borderless)
                    .foregroundStyle(JetDesign.accentText)
                    .disabled(model.stoppingID != nil)
                    .accessibilityIdentifier("schedule-stop-\(schedule.id.uuidString.lowercased())")
            }
        }
        .padding(.vertical, 6)
    }

    // MARK: New Daily Message

    @ViewBuilder
    private var newMessage: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: JetDesign.smallGap) {
            Text("New Daily Message")
                .font(.headline)
            TextField(
                "Message",
                text: $model.message,
                prompt: Text("What should \(assistant) do each day?"),
                axis: .vertical
            )
            .lineLimit(3 ... 8)
            .textFieldStyle(.roundedBorder)
            .labelsHidden()
            if model.showsByteCount {
                Text("\(model.messageBytes.formatted()) of \(RepeatDailyModel.maximumBytes.formatted()) bytes")
                    .font(.caption)
                    .monospacedDigit()
                    .foregroundStyle(model.messageBytes > RepeatDailyModel.maximumBytes ? .red : .secondary)
            }
            HStack(spacing: JetDesign.smallGap) {
                DatePicker(
                    "At",
                    selection: Binding(
                        get: { model.time(in: zone) },
                        set: { model.setTime($0, in: zone) }
                    ),
                    displayedComponents: .hourAndMinute
                )
                .environment(\.timeZone, zone)
                .fixedSize()
                Button(ScheduleCopy.zoneLabel(model.timeZoneID)) { showsTimeZones = true }
                    .help("Time Zone")
                    .popover(isPresented: $showsTimeZones, arrowEdge: .bottom) {
                        TimeZonePicker(selection: $model.timeZoneID) { showsTimeZones = false }
                    }
            }
            if hasNoRuns {
                Text("The first message waits until this task has started.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    // MARK: Actions

    private func save() {
        Task {
            if let notice = await model.save() {
                finish(notice)
            }
        }
    }

    private func finish(_ notice: String) {
        session.dismissSheet()
        session.composerNotice = ComposerNotice(kind: .confirmation, text: notice)
    }

    private func perform(_ action: LibraryIssue.Action) {
        switch action {
        case .tryAgain: Task { await model.load() }
        case .sendSameRequestAgain:
            Task {
                if let notice = await model.sendSameRequestAgain() { finish(notice) }
            }
        case let .review(pane): session.perform(.openSettings(pane))
        default: break
        }
    }
}

/// The time zone list: the current zone first, searchable by city or name.
private struct TimeZonePicker: View {
    @Binding var selection: String
    let done: () -> Void
    @State private var search = ""

    private var zones: [String] {
        let current = TimeZone.current.identifier
        let all = [current] + TimeZone.knownTimeZoneIdentifiers.filter { $0 != current }
        let query = search.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return all }
        return all.filter { ScheduleCopy.zoneLabel($0).localizedCaseInsensitiveContains(query) || $0.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: JetDesign.smallGap) {
            TextField("Search Time Zones", text: $search)
                .textFieldStyle(.roundedBorder)
            List(zones, id: \.self) { zone in
                Button {
                    selection = zone
                    done()
                } label: {
                    HStack {
                        Text(ScheduleCopy.zoneLabel(zone))
                        Spacer()
                        if zone == selection {
                            Image(systemName: "checkmark")
                                .foregroundStyle(JetDesign.accentText)
                                .accessibilityLabel(Text("Selected"))
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .listStyle(.plain)
        }
        .padding(JetDesign.smallGap)
        .frame(width: 320, height: 360)
    }
}

#if DEBUG
#Preview("Repeat Daily") {
    DesktopPreviewScenes.view("library-repeat-daily")
        .frame(width: 520, height: 620)
}
#endif
