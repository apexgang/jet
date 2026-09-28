import Foundation

// Add Project and New Task helpers (design §6.6, §6.7). They go through the
// same typed protocol calls as the rest of the session; errors are re-thrown
// unchanged so the sheet can map them.
extension DesktopSession {
    // MARK: - Projects per computer

    /// The projects a computer reported. This Mac falls back to its setup snapshot.
    func projects(on planeRegistryID: UUID) -> [JetProjectSummary] {
        planeSetupSnapshot(for: planeRegistryID)?.projects.projects ?? []
    }

    /// Asks a computer whether a folder can become a project.
    func previewProjectFolder(path: String, on planeRegistryID: UUID) async throws -> JetProjectPreview {
        try await client(for: planeRegistryID).previewProject(path: path)
    }

    /// Registers a reviewed folder with a caller-owned Command ID, so a deliberate
    /// retry of an uncertain registration stays the same Command.
    func registerProjectFolder(
        _ preview: JetProjectPreview,
        on planeRegistryID: UUID,
        commandID: UUID
    ) async throws -> JetProjectSummary {
        let project = try await client(for: planeRegistryID)
            .registerProject(preview: preview, commandID: commandID)
        await refreshProjects(on: planeRegistryID)
        return project
    }

    /// Reloads a computer's projects. Failures are recorded on the computer by the
    /// snapshot load and never surface here.
    func refreshProjects(on planeRegistryID: UUID) async {
        guard usesLivePlane, !isPreviewSession else { return }
        if isLocalPlane(planeRegistryID) {
            await loadSetup()
        } else {
            await loadPlaneSnapshot(planeRegistryID)
        }
    }

    // MARK: - New Task choices

    /// Chooses the project (and with it the computer) for the next task. It never
    /// navigates, so the project page isn't opened.
    func chooseNewTaskProject(_ projectID: UUID, on planeRegistryID: UUID) {
        guard projects(on: planeRegistryID).contains(where: { $0.id == projectID }) else { return }
        if newTaskPlaneRegistryID != planeRegistryID {
            newTaskPlaneRegistryID = planeRegistryID
            keepAvailableAssistant()
        }
        selectedProjectID = projectID
    }

    /// Chooses the computer for the next task. Its first project becomes the
    /// project, and the composer says so.
    func chooseNewTaskComputer(_ planeRegistryID: UUID) {
        let previous = newTaskPlaneRegistryID
        chooseNewTaskPlane(planeRegistryID)
        guard previous != planeRegistryID else { return }
        keepAvailableAssistant()
        if let project = selectedProject {
            composerNotice = ComposerNotice(
                kind: .confirmation,
                text: String(localized: "Project changed to \(project.name) on \(planeName(planeRegistryID)).")
            )
        }
    }

    /// Opens New Task in a project and moves focus to the composer. The project
    /// changes only after any unsaved file edit is resolved.
    func startNewTask(in projectID: UUID, on planeRegistryID: UUID, notice: String? = nil) {
        guardUnsavedEdits { [weak self] in
            guard let self else { return }
            if sidebarSelection != .newTask { open(.newTask) }
            chooseNewTaskProject(projectID, on: planeRegistryID)
            guard sidebarSelection == .newTask else { return }
            if let notice {
                composerNotice = ComposerNotice(kind: .confirmation, text: notice)
            }
            composerFocusRequest += 1
        }
    }

    /// A chosen assistant that the new computer doesn't have falls back to its first.
    private func keepAvailableAssistant() {
        guard let chosenCraftID,
              selectedSetupSnapshot?.capabilities.crafts.contains(where: { $0.id == chosenCraftID }) != true
        else { return }
        self.chosenCraftID = nil
    }
}
