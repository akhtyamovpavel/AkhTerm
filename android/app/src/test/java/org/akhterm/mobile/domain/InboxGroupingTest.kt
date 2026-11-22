package org.akhterm.mobile.domain

import org.akhterm.mobile.data.model.PaneState
import org.akhterm.mobile.data.model.RemotePaneSummary
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class InboxGroupingTest {

    @Test
    fun returnsEmptyListWhenNoPanes() {
        assertTrue(InboxGrouping.groupPanes(emptyList()).isEmpty())
    }

    @Test
    fun groupsSectionsInStrictAttentionPriorityOrder() {
        val panes = listOf(
            pane(id = "5", state = PaneState.IDLE),
            pane(id = "4", state = PaneState.DONE),
            pane(id = "3", state = PaneState.WORKING),
            pane(id = "2", state = PaneState.ERROR),
            pane(id = "1", state = PaneState.WAITING),
        )

        val sections = InboxGrouping.groupPanes(panes)
        assertEquals(
            listOf(
                InboxSectionKind.NEEDS_APPROVAL,
                InboxSectionKind.ERROR,
                InboxSectionKind.WORKING,
                InboxSectionKind.DONE,
                InboxSectionKind.IDLE,
            ),
            sections.map { it.kind },
        )
        assertEquals("1", sections[0].panes.single().paneId)
        assertEquals("2", sections[1].panes.single().paneId)
        assertEquals("3", sections[2].panes.single().paneId)
        assertEquals("4", sections[3].panes.single().paneId)
        assertEquals("5", sections[4].panes.single().paneId)
    }

    @Test
    fun omitsEmptySectionsAndKeepsStandardFourBucketOrder() {
        val panes = listOf(
            pane(id = "10", state = PaneState.IDLE),
            pane(id = "8", state = PaneState.DONE),
            pane(id = "6", state = PaneState.WORKING),
            pane(id = "4", state = PaneState.WAITING),
        )

        val sections = InboxGrouping.groupPanes(panes)
        assertEquals(
            listOf(
                InboxSectionKind.NEEDS_APPROVAL,
                InboxSectionKind.WORKING,
                InboxSectionKind.DONE,
                InboxSectionKind.IDLE,
            ),
            sections.map { it.kind },
        )
    }

    @Test
    fun sortsWithinSectionByUnreadActiveFocusedTabAndNumericPaneId() {
        val panes = listOf(
            pane(id = "10", tabId = 2, state = PaneState.WAITING, unread = false, activeTab = false, focusedPane = false),
            pane(id = "2", tabId = 2, state = PaneState.WAITING, unread = false, activeTab = false, focusedPane = false),
            pane(id = "7", tabId = 3, state = PaneState.WAITING, unread = true, activeTab = false, focusedPane = false),
            pane(id = "3", tabId = 1, state = PaneState.WAITING, unread = true, activeTab = true, focusedPane = false),
            pane(id = "4", tabId = 1, state = PaneState.WAITING, unread = true, activeTab = true, focusedPane = true),
        )

        val section = InboxGrouping.groupPanes(panes).single()
        assertEquals(InboxSectionKind.NEEDS_APPROVAL, section.kind)
        assertEquals(
            listOf("4", "3", "7", "2", "10"),
            section.panes.map { it.paneId },
        )
    }

    private fun pane(
        id: String,
        tabId: Long = 1L,
        state: PaneState,
        unread: Boolean = false,
        activeTab: Boolean = false,
        focusedPane: Boolean = false,
    ): RemotePaneSummary = RemotePaneSummary(
        paneId = id,
        tabId = tabId,
        tabTitle = "tab-$tabId",
        activeTab = activeTab,
        focusedPane = focusedPane,
        agent = "Claude Code",
        repoName = "akhterm-rs",
        branchBadge = "main",
        isWorktree = false,
        state = state,
        unread = unread,
    )
}
