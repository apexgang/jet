import SwiftUI

struct SidebarView: View {
    @Bindable var session: DesktopSession
    @FocusState private var searchFocused: Bool
#if os(macOS)
    @Environment(\.openSettings) private var openSettings
#endif

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                JetMark(size: 24)
                Text("Jet").font(.title3.weight(.semibold))
                Spacer()
                Text("Workspace").font(.caption2).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 20).padding(.vertical, 16)
            VStack(spacing: 8) {
                Button(action: session.beginNewTask) {
                    HStack { Text("New task"); Spacer(); Text("⌘N").foregroundStyle(.secondary).font(.caption) }
                        .padding(.vertical, 4)
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
                Button {
                    session.selectSearch()
                    searchFocused = true
                } label: {
                    HStack { Label("Find a task", systemImage: "magnifyingglass"); Spacer(); Text("⌘K").font(.caption) }
                }
                .buttonStyle(.plain).foregroundStyle(.secondary).padding(8)
                if session.sidebarSelection == .search {
                    TextField("Search your work", text: $session.searchText)
                        .textFieldStyle(.roundedBorder).focused($searchFocused)
                        .onSubmit { Task { await session.searchConversations() } }
                }
            }
            .padding(.horizontal, 12)
            List {
                if session.attentionCount > 0 {
                    Button {
                        session.sidebarSelection = .needsAttention
                        session.applySidebarSelection()
                    } label: {
                        HStack { Text("Needs attention"); Spacer(); Text(session.attentionCount, format: .number) }
                            .foregroundStyle(.orange)
                    }
                }
                if session.sidebarSelection == .search, let result = session.searchResult {
                    Section("Search results") {
                        ForEach(result.hits) { hit in
                            Button { session.selectSearchHit(hit) } label: {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(hit.hit.excerpt).lineLimit(3)
                                    Text(hit.planeName).font(.caption2).foregroundStyle(.secondary)
                                }
                            }.buttonStyle(.plain)
                        }
                        if result.hits.isEmpty { Text("No matching tasks").foregroundStyle(.secondary) }
                    }
                }
                Section("Your tasks") {
                    ForEach(session.conversations) { conversation in
                        Button { session.selectConversation(conversation.id) } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(conversation.title).lineLimit(2)
                                if session.planes.count > 1 {
                                    Text(session.conversationPlaneName(conversation.id)).font(.caption2).foregroundStyle(.secondary)
                                }
                            }
                            .padding(.vertical, 5).frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(session.selectedConversationID == conversation.id ? JetDesign.accent.opacity(0.12) : Color.clear)
                        .accessibilityValue(session.selectedConversationID == conversation.id ? "Selected" : "")
                    }
                    if session.conversations.isEmpty {
                        Text("Your work will be saved here.").font(.caption).foregroundStyle(.secondary)
                    }
                    if session.hasMoreConversations {
                        Button("Show earlier tasks") { Task { await session.loadMoreConversations() } }
                            .disabled(session.conversationOperation != nil)
                    }
                }
            }
            .listStyle(.sidebar)
            VStack(spacing: 12) {
                HStack(spacing: 16) {
                    Button("Projects", action: session.showProjects)
#if os(macOS)
                    Button("Schedules") { session.requestSettings(.work); openSettings() }
                    Button("Settings") { openSettings() }
#endif
                }.buttonStyle(.plain).font(.caption).foregroundStyle(.secondary)
                Button {
                    session.sidebarSelection = .planes
                    session.applySidebarSelection()
                } label: { PlaneStatusFooter(session: session) }
                .buttonStyle(.plain).help("Planes are computers running Jet")
            }
            .padding(.top, 12)
        }
        .navigationTitle("Jet")
        .onChange(of: session.sidebarSelection) { _, value in
            if value == .search { searchFocused = true }
        }
    }
}

private struct PlaneStatusFooter: View {
    let session: DesktopSession

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(connectionColor)
                .frame(width: 7, height: 7)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text("This Mac")
                    .font(.caption.weight(.medium))
                Text(session.planeConnectionLabel)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if !session.remoteProfiles.isEmpty {
                Text("+\(session.remoteProfiles.count) remote")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .modifier(LegibleBarBackground())
        .accessibilityElement(children: .combine)
    }

    private var connectionColor: Color {
        switch session.connectionState {
        case .connected: .green
        case .connecting, .reconnecting: .orange
        case .failed: .red
        case .disconnected: .secondary
        }
    }
}
