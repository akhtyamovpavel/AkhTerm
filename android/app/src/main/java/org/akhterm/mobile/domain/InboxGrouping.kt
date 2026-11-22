package org.akhterm.mobile.domain

import org.akhterm.mobile.data.model.PaneState
import org.akhterm.mobile.data.model.RemotePaneSummary

enum class InboxSectionKind(val priority: Int) {
    NEEDS_APPROVAL(0),
    ERROR(1),
    WORKING(2),
    DONE(3),
    IDLE(4),
}

data class InboxSection(
    val kind: InboxSectionKind,
    val panes: List<RemotePaneSummary>,
)

object InboxGrouping {
    fun sectionFor(state: PaneState): InboxSectionKind = when (state) {
        PaneState.WAITING -> InboxSectionKind.NEEDS_APPROVAL
        PaneState.ERROR -> InboxSectionKind.ERROR
        PaneState.WORKING -> InboxSectionKind.WORKING
        PaneState.DONE -> InboxSectionKind.DONE
        PaneState.IDLE -> InboxSectionKind.IDLE
    }

    /**
     * Groups panes by attention priority:
     * Needs Approval -> Error -> Working -> Done -> Idle.
     * Empty sections are omitted. Within each section, unread panes come first,
     * followed by active tab, focused pane, tab order, and numeric pane id.
     */
    fun groupPanes(panes: List<RemotePaneSummary>): List<InboxSection> {
        if (panes.isEmpty()) return emptyList()

        val grouped = panes.groupBy { sectionFor(it.state) }
        return InboxSectionKind.entries
            .sortedBy { it.priority }
            .mapNotNull { kind ->
                val items = grouped[kind]?.sortedWith(PANE_COMPARATOR)
                if (items.isNullOrEmpty()) null else InboxSection(kind = kind, panes = items)
            }
    }

    private val PANE_COMPARATOR = Comparator<RemotePaneSummary> { a, b ->
        compareValuesBy(
            a,
            b,
            { !it.unread },
            { !it.activeTab },
            { !it.focusedPane },
            { it.tabId },
            { it.paneId.toLongOrNull() ?: Long.MAX_VALUE },
            { it.paneId },
        )
    }
}
