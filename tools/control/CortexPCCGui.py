#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import queue
import re
import subprocess
import sys
import threading
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Sequence

from PCCSurfaceCommon import (
    BackendClient,
    ProjectContract,
    ProjectRegistry,
    RegisteredProject,
    SurfaceCommand,
    SurfaceError,
    command_catalog,
    compact_path,
    latest_debug_bundle,
    open_path,
    resolve_root,
    reveal_file,
    terminate_process_tree,
    validate_surface,
)

from PCCVaultCatalog import (
    baseline_dir as vault_baseline_dir,
    capture_baseline as vault_capture_baseline,
    compare_baseline as vault_compare_baseline,
    catalog_dir as vault_catalog_dir,
    catalog_record as vault_catalog_record,
    classify_path as vault_classify_path,
    latest_summary as vault_latest_summary,
    scan_project as vault_scan_project,
    search_catalog as vault_search_catalog,
    vault_root as global_vault_root,
)
from CortexPersistentVolumeInventory import (
    index_location as persistent_volume_location,
    index_status as persistent_volume_status,
    query_entries as persistent_volume_query,
    run_scan as persistent_volume_scan,
    list_errors as persistent_volume_errors,
)
from PCCRepoHygiene import prepare as repo_hygiene_prepare
from PCCVaultIntakeAudit import (
    audit_dir as vault_intake_audit_dir,
    audit_root as vault_intake_root,
    create_handoff as vault_intake_create_handoff,
    latest_summary as vault_intake_latest_summary,
    scan_intake as vault_intake_scan,
)
from PCCChatStore import list_conversations as chat_list_conversations, load_messages as chat_load_messages
from PCCStoragePaths import project_vault_dir as vault_project_dir
from PCCVaultStorage import (
    dependency_status as vault_dependency_status,
    mirror_project as vault_mirror_project,
    mirror_status as vault_mirror_status,
    reclaim_plan as vault_reclaim_plan,
    storage_health as vault_storage_health,
    snapshot_retention_plan as vault_retention_plan,
    cas_gc_plan as vault_cas_gc_plan,
    verify_latest_mirror as vault_verify_latest_mirror,
)

GUI_VERSION = "PCC-GUI-0.15.0"

BG = "#090b0e"
PANEL = "#11151a"
PANEL_2 = "#171c22"
BORDER = "#28313a"
TEXT = "#edf2f5"
MUTED = "#929aa3"
CYAN = "#00d9ff"
GREEN = "#43f071"
YELLOW = "#ffd44a"
RED = "#ff5d68"


class CortexPCCGui:
    def __init__(self, root: Path) -> None:
        import tkinter as tk
        from tkinter import filedialog, messagebox, simpledialog, ttk

        self.tk = tk
        self.ttk = ttk
        self.filedialog = filedialog
        self.messagebox = messagebox
        self.simpledialog = simpledialog

        self.registry = ProjectRegistry()
        self.root_path = root.resolve()
        self._startup_hygiene: dict[str, Any] = {}
        try:
            self._startup_hygiene = repo_hygiene_prepare(self.root_path, apply=False)
        except Exception as exc:
            self._startup_hygiene = {"moved": 0, "error": str(exc)}
        self.contract = ProjectContract.load(self.root_path)
        self.backend: BackendClient | None = None
        self.backend_error = ""
        self._bind_project_backend()
        self.registry.touch(self.root_path)

        self.window = tk.Tk()
        self.window.title(f"Project Control Center — {self.contract.name}")
        self.window.geometry("1280x860")
        self.window.minsize(1040, 720)
        self.window.configure(bg=BG)
        self.window.protocol("WM_DELETE_WINDOW", self._on_close)

        self._event_q: queue.Queue[tuple[str, Any]] = queue.Queue()
        self._active_proc: subprocess.Popen[str] | None = None
        self._active_command = ""
        self._last_status: dict[str, Any] = {}
        self._page_frames: dict[str, Any] = {}
        self._page_bodies: dict[str, Any] = {}
        self._page_canvases: dict[str, Any] = {}
        self._active_page = ""
        self._status_values: dict[str, Any] = {}
        self._status_leds: dict[str, tuple[Any, Any]] = {}
        self._nav_buttons: dict[str, Any] = {}
        self._app_frames: dict[str, Any] = {}
        self._app_tab_buttons: dict[str, Any] = {}
        self._project_entries_by_id: dict[str, RegisteredProject] = {}
        self._busy = False
        self._command_rows: list[SurfaceCommand] = command_catalog(self.contract)
        self._category_command_hosts: list[tuple[Any, tuple[str, ...]]] = []
        self._console_history: list[str] = []
        self._console_history_index: int | None = None
        self._cortex_conversations: dict[str, str] = {}
        self._chat_rendered_conversation: str | None = None
        self._chat_active_buffer: list[str] = []
        self._chat_conversation_rows: dict[str, str] = {}
        self._project_probe_generation = 0
        self._project_probe_after: str | None = None
        # Status reads are asynchronous and may overlap startup/operation completion.
        # Only the newest generation may update the authoritative header/dashboard.
        self._status_generation = 0
        self._vault_tree_initialized = False
        self._console_compacting_diff = False
        self._vault_busy = False
        self._vault_cancel = False
        self._vault_node_paths: dict[str, Path] = {}
        self._vault_metrics: dict[str, Any] = {}
        self._volume_result: dict[str, Any] | None = None
        self._volume_running = False
        self._volume_view_generation = 0
        self._volume_root: Path | None = None
        self._volume_db_path: Path | None = None
        self._volume_id: str | None = None
        self._volume_page = 0
        self._volume_page_size = 200
        self._volume_progress_last_query = 0.0
        self._volume_filter_busy = False
        self._volume_filter_pending = False

        self._configure_styles()
        self._build_shell()
        self.window.bind("<MouseWheel>", self._scroll_active_page, add="+")
        self.window.bind("<Prior>", lambda event: self._scroll_active_page_key(event, -1, "pages"), add="+")
        self.window.bind("<Next>", lambda event: self._scroll_active_page_key(event, 1, "pages"), add="+")
        self.window.bind("<Home>", lambda event: self._scroll_active_page_home_end(event, 0.0), add="+")
        self.window.bind("<End>", lambda event: self._scroll_active_page_home_end(event, 1.0), add="+")
        self.window.bind("<Up>", lambda event: self._scroll_active_page_key(event, -1, "units"), add="+")
        self.window.bind("<Down>", lambda event: self._scroll_active_page_key(event, 1, "units"), add="+")
        self._refresh_projects()
        self._show_page("Dashboard")
        self._show_app_tab("Projects")
        self._append_log(f"[PASS] Project Control Center {GUI_VERSION} ACTIVE.\n", "pass")
        self._append_log(f"Active project: {self.contract.name} — {self.root_path}\n", "muted")
        moved = int(self._startup_hygiene.get("moved", 0) or 0)
        if self._startup_hygiene.get("error"):
            self._append_log(f"[WARN] Startup repository hygiene could not complete: {self._startup_hygiene['error']}\n", "warn")
        elif moved:
            self._append_log(f"[PASS] Startup repository hygiene preview: {moved} loose operational artifact(s) would move during an explicit operation.\n", "pass")
        else:
            self._append_log("[PASS] Startup repository hygiene: root transport area clean.\n", "pass")
        self._refresh_status_async()
        self.window.after(60, self._drain_events)

    # ------------------------------------------------------------------
    # Shell / styling
    # ------------------------------------------------------------------
    def _configure_styles(self) -> None:
        ttk = self.ttk
        style = ttk.Style(self.window)
        try:
            style.theme_use("clam")
        except Exception:
            pass
        style.configure(
            "Treeview",
            background=PANEL_2,
            fieldbackground=PANEL_2,
            foreground=TEXT,
            rowheight=29,
            borderwidth=0,
        )
        style.configure("Treeview.Heading", background=PANEL, foreground=CYAN, relief="flat")
        style.map("Treeview", background=[("selected", "#21404a")], foreground=[("selected", TEXT)])
        style.configure("TProgressbar", troughcolor=PANEL_2, background=CYAN, borderwidth=0)
        style.configure(
            "Dark.Vertical.TScrollbar",
            troughcolor="#090c10",
            background="#35424e",
            bordercolor="#090c10",
            darkcolor="#35424e",
            lightcolor="#35424e",
            arrowcolor=TEXT,
            relief="flat",
            borderwidth=0,
            arrowsize=14,
            width=16,
        )
        style.map(
            "Dark.Vertical.TScrollbar",
            background=[("active", "#536675"), ("pressed", CYAN)],
            arrowcolor=[("active", CYAN), ("pressed", BG)],
        )
        style.configure(
            "Dark.Horizontal.TScrollbar",
            troughcolor="#090c10",
            background="#35424e",
            bordercolor="#090c10",
            darkcolor="#35424e",
            lightcolor="#35424e",
            arrowcolor=TEXT,
            relief="flat",
            borderwidth=0,
            arrowsize=14,
            width=16,
        )
        style.map(
            "Dark.Horizontal.TScrollbar",
            background=[("active", "#536675"), ("pressed", CYAN)],
            arrowcolor=[("active", CYAN), ("pressed", BG)],
        )

    def _dark_scrollbar(self, parent: Any, *, orient: str, command: Callable[..., Any]) -> Any:
        style = "Dark.Vertical.TScrollbar" if orient == "vertical" else "Dark.Horizontal.TScrollbar"
        return self.ttk.Scrollbar(parent, orient=orient, command=command, style=style, takefocus=True)


    def _build_shell(self) -> None:
        tk = self.tk

        header = tk.Frame(self.window, bg=BG, height=84)
        header.pack(fill="x", padx=22, pady=(12, 5))
        header.pack_propagate(False)

        title_block = tk.Frame(header, bg=BG)
        title_block.pack(side="left", fill="y")
        tk.Label(
            title_block,
            text="PROJECT CONTROL CENTER",
            bg=BG,
            fg=CYAN,
            font=("Segoe UI Semibold", 18),
        ).pack(anchor="w")
        self.active_project_label = tk.Label(
            title_block,
            text="",
            bg=BG,
            fg=MUTED,
            font=("Segoe UI", 9),
        )
        self.active_project_label.pack(anchor="w", pady=(4, 0))
        self._update_header()

        header_actions = tk.Frame(header, bg=BG)
        header_actions.pack(side="right", fill="y")
        tk.Label(
            header_actions,
            text=f"{GUI_VERSION} ACTIVE",
            bg=BG,
            fg=GREEN,
            font=("Consolas", 8),
        ).pack(side="left", padx=(0, 9), pady=13)
        self.refresh_btn = self._button(header_actions, "Refresh", self._refresh_clicked, compact=True)
        self.refresh_btn.pack(side="left", padx=3, pady=13)
        self.cli_btn = self._button(header_actions, "Open Project CLI", self._open_cli, compact=True)
        self.cli_btn.pack(side="left", padx=3, pady=13)

        # Workspace health occupies the free header space only while Project Workspace is active.
        self.header_health_host = tk.Frame(header, bg=BG)
        self.header_health_host.pack(side="left", fill="both", expand=True, padx=(28, 16))
        self.header_health_rail = tk.Frame(self.header_health_host, bg=BG)
        self._build_header_health_rail(self.header_health_rail)

        tk.Frame(self.window, bg=CYAN, height=1).pack(fill="x")

        tabs = tk.Frame(self.window, bg=PANEL, height=44)
        tabs.pack(fill="x")
        tabs.pack_propagate(False)

        for name in ("Projects", "Chat", "Project Workspace"):
            btn = tk.Button(
                tabs,
                text=name,
                command=lambda n=name: self._show_app_tab(n),
                bg=PANEL,
                fg=TEXT,
                activebackground=PANEL_2,
                activeforeground=CYAN,
                bd=0,
                relief="flat",
                cursor="hand2",
                font=("Segoe UI Semibold", 10),
                padx=20,
                pady=8,
            )
            btn.pack(side="left", fill="y")
            self._app_tab_buttons[name] = btn

        vault_btn = tk.Button(
            tabs,
            text="Vault / Forge",
            command=lambda: self._show_app_tab("Vault / Forge"),
            bg=PANEL,
            fg=TEXT,
            activebackground=PANEL_2,
            activeforeground=CYAN,
            bd=0,
            relief="flat",
            cursor="hand2",
            font=("Segoe UI Semibold", 10),
            padx=22,
            pady=8,
        )
        vault_btn.pack(side="right", fill="y")
        self._app_tab_buttons["Vault / Forge"] = vault_btn

        self.app_content = tk.Frame(self.window, bg=BG)
        self.app_content.pack(fill="both", expand=True)
        for name in ("Projects", "Chat", "Project Workspace", "Vault / Forge"):
            frame = tk.Frame(self.app_content, bg=BG)
            self._app_frames[name] = frame

        self._build_projects_tab(self._app_frames["Projects"])
        self._build_chat_tab(self._app_frames["Chat"])
        self._build_workspace_tab(self._app_frames["Project Workspace"])
        self._build_vault_tab(self._app_frames["Vault / Forge"])

    def _build_projects_tab(self, parent: Any) -> None:
        tk = self.tk
        ttk = self.ttk

        shell = tk.Frame(parent, bg=BG)
        shell.pack(fill="both", expand=True, padx=20, pady=18)
        self._section_title(
            shell,
            "Registered Projects",
            "Universal PCC registry. Select a project to load its Project Workspace.",
        )

        toolbar = tk.Frame(shell, bg=BG)
        toolbar.pack(fill="x", pady=(0, 10))
        self._toolbar_grid(toolbar, (
            ("Register Project...", self._register_project, True, False),
            ("Open Project Workspace", self._open_selected_project, False, False),
            ("Open Folder", self._open_selected_project_folder, False, False),
            ("Remove Registration", self._remove_selected_project, False, True),
            ("Refresh", self._refresh_projects, False, False),
        ), preferred_columns=5, minimum_cell_width=150)

        panel = self._panel(shell)
        panel.pack(fill="both", expand=True)
        self.projects_tree = ttk.Treeview(
            panel,
            columns=("name", "kind", "root", "adapter", "catalog", "last"),
            show="headings",
            selectmode="browse",
        )
        for key, title, width in (
            ("name", "Project", 190),
            ("kind", "Type", 140),
            ("root", "Repository / Root", 520),
            ("adapter", "PCC", 130),
            ("catalog", "Vault", 110),
            ("last", "Last Opened", 160),
        ):
            self.projects_tree.heading(key, text=title)
            self.projects_tree.column(key, width=width, anchor="w")
        self.projects_tree.tag_configure("ready", foreground=GREEN)
        self.projects_tree.tag_configure("adapter", foreground=YELLOW)
        self.projects_tree.tag_configure("missing", foreground=RED)
        scroll = self._dark_scrollbar(panel, orient="vertical", command=self.projects_tree.yview)
        self.projects_tree.configure(yscrollcommand=scroll.set)
        self.projects_tree.pack(side="left", fill="both", expand=True, padx=(10, 0), pady=10)
        scroll.pack(side="right", fill="y", padx=(0, 8), pady=10)
        self.projects_tree.bind("<<TreeviewSelect>>", self._project_selection_changed)
        self.projects_tree.bind("<Double-1>", lambda _e: self._open_selected_project())

        detail = self._panel(shell, "Selected Project")
        detail.pack(fill="x", pady=(10, 0))
        self.project_detail = tk.Label(
            detail,
            text="Select a registered project.",
            bg=PANEL,
            fg=MUTED,
            justify="left",
            anchor="w",
            font=("Consolas", 9),
        )
        self.project_detail.pack(fill="x", padx=14, pady=(4, 12))


    def _build_chat_tab(self, parent: Any) -> None:
        tk = self.tk
        ttk = self.ttk

        shell = tk.Frame(parent, bg=BG)
        shell.pack(fill="both", expand=True, padx=16, pady=(12, 8))

        header = self._panel(shell)
        header.pack(fill="x", pady=(0, 8))
        header_left = tk.Frame(header, bg=PANEL)
        header_left.pack(side="left", fill="both", expand=True, padx=12, pady=10)
        tk.Label(
            header_left,
            text="CORTEX CHAT",
            bg=PANEL,
            fg=CYAN,
            font=("Segoe UI Semibold", 11),
        ).pack(anchor="w")
        self.chat_project_label = tk.Label(
            header_left,
            text="",
            bg=PANEL,
            fg=MUTED,
            font=("Segoe UI", 8),
            anchor="w",
            justify="left",
        )
        self.chat_project_label.pack(anchor="w", pady=(3, 0))

        header_right = tk.Frame(header, bg=PANEL)
        header_right.pack(side="right", padx=10, pady=8)
        self.chat_worker_label = tk.Label(
            header_right,
            text="Python control plane ready",
            bg=PANEL,
            fg=GREEN,
            font=("Segoe UI Semibold", 8),
        )
        self.chat_worker_label.pack(side="left", padx=(0, 8))
        self._button(header_right, "New Chat", self._new_cortex_conversation, compact=True).pack(side="left", padx=3)
        self._button(header_right, "Repair Plan", self._start_repair_plan, compact=True).pack(side="left", padx=3)
        self._button(header_right, "Repair + FULL", self._start_repair_current_full, compact=True).pack(side="left", padx=3)
        self._button(header_right, "Build Worker", self._start_cortex_worker_build, compact=True).pack(side="left", padx=3)

        panes = tk.PanedWindow(
            shell,
            orient="horizontal",
            bg=BG,
            bd=0,
            sashwidth=5,
            sashrelief="flat",
            showhandle=False,
            opaqueresize=True,
        )
        panes.pack(fill="both", expand=True)

        history = self._panel(panes)
        history.configure(width=240)
        history.pack_propagate(False)
        tk.Label(
            history,
            text="CONVERSATIONS",
            bg=PANEL,
            fg=MUTED,
            font=("Segoe UI Semibold", 8),
        ).pack(anchor="w", padx=10, pady=(10, 6))
        history_body = tk.Frame(history, bg=PANEL)
        history_body.pack(fill="both", expand=True, padx=7, pady=(0, 7))
        self.chat_history_tree = ttk.Treeview(
            history_body,
            columns=("title", "messages"),
            show="headings",
            selectmode="browse",
            height=10,
        )
        self.chat_history_tree.heading("title", text="Conversation")
        self.chat_history_tree.heading("messages", text="#")
        self.chat_history_tree.column("title", width=185, anchor="w")
        self.chat_history_tree.column("messages", width=34, anchor="center")
        hscroll = self._dark_scrollbar(history_body, orient="vertical", command=self.chat_history_tree.yview)
        self.chat_history_tree.configure(yscrollcommand=hscroll.set)
        self.chat_history_tree.pack(side="left", fill="both", expand=True)
        hscroll.pack(side="right", fill="y")
        self.chat_history_tree.bind("<<TreeviewSelect>>", self._chat_history_selected)
        self._button(history, "Refresh History", self._refresh_chat_history, compact=True).pack(fill="x", padx=8, pady=(0, 8))

        chat = self._panel(panes)
        transcript_body = tk.Frame(chat, bg="#0b0e12")
        transcript_body.pack(fill="both", expand=True, padx=8, pady=(8, 4))
        self.chat_text = tk.Text(
            transcript_body,
            bg="#0b0e12",
            fg=TEXT,
            insertbackground=TEXT,
            selectbackground="#21404a",
            bd=0,
            relief="flat",
            wrap="word",
            font=("Segoe UI", 10),
            padx=18,
            pady=14,
            state="disabled",
        )
        self.chat_text.tag_configure("chat-user-head", foreground=CYAN, font=("Segoe UI Semibold", 9), spacing1=10)
        self.chat_text.tag_configure("chat-assistant-head", foreground=GREEN, font=("Segoe UI Semibold", 9), spacing1=10)
        self.chat_text.tag_configure("chat-system-head", foreground=YELLOW, font=("Segoe UI Semibold", 9), spacing1=10)
        self.chat_text.tag_configure("chat-body", foreground=TEXT, font=("Segoe UI", 10), lmargin1=4, lmargin2=4, spacing3=10)
        self.chat_text.tag_configure("chat-muted", foreground=MUTED, font=("Segoe UI", 9), spacing3=8)
        tscroll = self._dark_scrollbar(transcript_body, orient="vertical", command=self.chat_text.yview)
        self.chat_text.configure(yscrollcommand=tscroll.set)
        self.chat_text.pack(side="left", fill="both", expand=True)
        tscroll.pack(side="right", fill="y")

        composer = tk.Frame(chat, bg=PANEL)
        composer.pack(fill="x", padx=8, pady=(4, 8))

        composer_top = tk.Frame(composer, bg=PANEL)
        composer_top.pack(fill="x", pady=(0, 5))
        tk.Label(composer_top, text="Mode", bg=PANEL, fg=MUTED, font=("Segoe UI", 8)).pack(side="left")
        self.chat_mode_var = tk.StringVar(value="Chat")
        self.chat_mode_box = ttk.Combobox(
            composer_top,
            textvariable=self.chat_mode_var,
            values=("Chat", "Inspect", "Plan", "Apply", "Repair"),
            state="readonly",
            width=12,
        )
        self.chat_mode_box.pack(side="left", padx=(6, 8))
        tk.Label(
            composer_top,
            text="Enter sends • Shift+Enter adds a line • Apply/Repair require the transactional worker",
            bg=PANEL,
            fg=MUTED,
            font=("Segoe UI", 8),
        ).pack(side="left")

        composer_bottom = tk.Frame(composer, bg=PANEL)
        composer_bottom.pack(fill="x")
        self.chat_input = tk.Text(
            composer_bottom,
            height=4,
            bg=PANEL_2,
            fg=TEXT,
            insertbackground=TEXT,
            selectbackground="#21404a",
            bd=1,
            relief="solid",
            highlightthickness=1,
            highlightbackground=BORDER,
            highlightcolor=CYAN,
            wrap="word",
            font=("Segoe UI", 10),
            padx=10,
            pady=8,
        )
        self.chat_input.pack(side="left", fill="x", expand=True, padx=(0, 8))
        self.chat_input.bind("<Return>", self._submit_chat_input)
        self.chat_input.bind("<Shift-Return>", self._chat_insert_newline)
        controls = tk.Frame(composer_bottom, bg=PANEL)
        controls.pack(side="right", fill="y")
        self.chat_send_btn = self._button(controls, "Send", self._submit_chat_input, primary=True, compact=True)
        self.chat_send_btn.pack(fill="x", pady=(0, 5))
        self.chat_stop_btn = self._button(controls, "Stop", self._stop_active, compact=True, danger=True)
        self.chat_stop_btn.pack(fill="x")
        self.chat_stop_btn.configure(state="disabled")

        panes.add(history, minsize=190, width=240)
        panes.add(chat, minsize=520)
        self.chat_panes = panes
        self._refresh_chat_header()
        self._refresh_chat_history()
        self._render_current_chat()


    def _build_workspace_tab(self, parent: Any) -> None:
        tk = self.tk

        # Quick actions span the workspace, but reflow instead of becoming a
        # permanently clipped horizontal strip on narrower windows.
        quick = self._panel(parent)
        quick.pack(fill="x", padx=16, pady=(10, 6))
        tk.Label(
            quick,
            text="QUICK ACTIONS",
            bg=PANEL,
            fg=CYAN,
            font=("Segoe UI Semibold", 9),
        ).pack(anchor="w", padx=12, pady=(9, 2))
        self._toolbar_grid(quick, (
            ("FULL GATE / CERTIFY GREEN", lambda: self._start_command("full"), True, False),
            ("COMMIT + PUSH GREEN", self._commit_push_green, False, False),
            ("BUILD", lambda: self._start_command("build"), False, False),
            ("RUN", lambda: self._start_command("launch-gui"), False, False),
            ("APPLY UPDATES", self._apply_updates, False, False),
            ("DEBUG BUNDLE", lambda: self._start_command("debug-bundle"), False, False),
        ), preferred_columns=6, minimum_cell_width=145)

        # Main Project Workspace is always three columns:
        #   small operation rail | dynamic command surface | persistent console.
        panes = tk.PanedWindow(
            parent,
            orient="horizontal",
            bg=BG,
            bd=0,
            sashwidth=5,
            sashrelief="flat",
            showhandle=False,
            opaqueresize=True,
        )
        panes.pack(fill="both", expand=True, padx=16, pady=(0, 6))
        self.workspace_panes = panes

        # LEFT: compact navigation authority.
        nav = self._panel(panes)
        nav.configure(width=158)
        nav.pack_propagate(False)
        nav_header = tk.Frame(nav, bg=PANEL)
        nav_header.pack(fill="x", padx=10, pady=(11, 5))
        tk.Label(
            nav_header,
            text="PROJECT OPERATIONS",
            bg=PANEL,
            fg=MUTED,
            font=("Segoe UI Semibold", 8),
        ).pack(anchor="w")

        nav_pages = (
            ("Dashboard", "Dashboard"),
            ("Build & Run", "Build & Run"),
            ("Updates", "Updates"),
            ("Source Control", "Source Control"),
            ("Diagnostics", "Diagnostics"),
            ("Storage & Vault", "Storage & Vault"),
            ("Tooling", "Tooling"),
            ("Advanced Commands", "Command Registry"),
        )
        for page, label in nav_pages:
            btn = tk.Button(
                nav,
                text=label,
                anchor="w",
                command=lambda p=page: self._show_page(p),
                bg=PANEL,
                fg=TEXT,
                activebackground=PANEL_2,
                activeforeground=CYAN,
                bd=0,
                relief="flat",
                font=("Segoe UI", 9),
                cursor="hand2",
                padx=12,
                pady=7,
            )
            btn.pack(fill="x", padx=3, pady=1)
            self._nav_buttons[page] = btn

        tk.Frame(nav, bg=BORDER, height=1).pack(fill="x", padx=10, pady=(10, 8))
        self.operation_label = tk.Label(
            nav,
            text="Idle",
            bg=PANEL,
            fg=MUTED,
            font=("Segoe UI", 8),
            wraplength=132,
            justify="left",
        )
        self.operation_label.pack(anchor="w", padx=12, pady=(0, 5))
        self.stop_btn = self._button(nav, "Stop Active Job", self._stop_active, compact=True, danger=True)
        # Hidden when idle; it appears only while an operation is actually running.
        self.stop_btn.configure(state="disabled")

        # MIDDLE: clicking the left rail swaps this dynamic command surface.
        center = self._panel(panes)
        self.content = tk.Frame(center, bg=PANEL)
        self.content.pack(fill="both", expand=True, padx=13, pady=12)

        pages = (
            "Dashboard", "Build & Run", "Updates", "Source Control", "Diagnostics",
            "Storage & Vault", "Tooling", "Advanced Commands",
        )
        scrollable_pages = {
            "Build & Run",
            "Updates",
            "Source Control",
            "Diagnostics",
            "Storage & Vault",
            "Tooling",
        }
        for page in pages:
            frame = tk.Frame(self.content, bg=PANEL)
            self._page_frames[page] = frame
            if page in scrollable_pages:
                self._page_bodies[page] = self._scrollable_page_body(frame, page)
            else:
                self._page_bodies[page] = frame

        self._build_dashboard(self._page_bodies["Dashboard"])
        self._build_build_page(self._page_bodies["Build & Run"])
        self._build_updates_page(self._page_bodies["Updates"])
        self._build_source_page(self._page_bodies["Source Control"])
        self._build_diagnostics_page(self._page_bodies["Diagnostics"])
        self._build_storage_commands_page(self._page_bodies["Storage & Vault"])
        self._build_tooling_page(self._page_bodies["Tooling"])
        self._build_commands_page(self._page_bodies["Advanced Commands"])

        # RIGHT: persistent console takes almost half the application by default.
        console = self._panel(panes)
        self.console_panel = console
        console_bar = tk.Frame(console, bg=PANEL)
        console_bar.pack(fill="x", padx=10, pady=(8, 5))
        tk.Label(
            console_bar,
            text="PROJECT CONSOLE",
            bg=PANEL,
            fg=CYAN,
            font=("Segoe UI Semibold", 9),
        ).pack(side="left")
        tk.Label(
            console_bar,
            text=GUI_VERSION,
            bg=PANEL,
            fg=MUTED,
            font=("Consolas", 7),
        ).pack(side="left", padx=(7, 0))
        self.console_job_label = tk.Label(
            console_bar,
            text="Idle",
            bg=PANEL,
            fg=MUTED,
            font=("Segoe UI", 8),
        )
        self.console_job_label.pack(side="left", padx=(9, 0))
        self._button(console_bar, "Copy All", lambda: self._copy_all(self.console_text), compact=True).pack(side="right", padx=(5, 0))
        self._button(console_bar, "Copy Sel", lambda: self._copy_selection(self.console_text), compact=True).pack(side="right", padx=(5, 0))
        self._button(console_bar, "Clear", self._clear_log, compact=True).pack(side="right", padx=(5, 0))
        self._button(console_bar, "Log", self._open_active_log, compact=True).pack(side="right", padx=(5, 0))

        console_composer = tk.Frame(console, bg=PANEL)
        console_composer.pack(side="bottom", fill="x", padx=8, pady=(0, 8))
        self.console_input_var = tk.StringVar()
        self.console_verbose_var = tk.BooleanVar(value=False)
        self.console_entry = tk.Entry(
            console_composer,
            textvariable=self.console_input_var,
            bg="#07090b",
            fg=TEXT,
            insertbackground=TEXT,
            selectbackground="#21404a",
            selectforeground=TEXT,
            bd=0,
            relief="flat",
            font=("Consolas", 9),
        )
        self.console_entry.pack(side="left", fill="x", expand=True, ipady=7, padx=(0, 6))
        self.console_entry.bind("<Return>", self._submit_console_input)
        self.console_entry.bind("<Up>", self._console_history_up)
        self.console_entry.bind("<Down>", self._console_history_down)
        self._button(console_composer, "Send", self._submit_console_input, primary=True, compact=True).pack(side="left", padx=(0, 6))
        tk.Checkbutton(
            console_composer,
            text="Verbose",
            variable=self.console_verbose_var,
            bg=PANEL,
            fg=MUTED,
            activebackground=PANEL,
            activeforeground=TEXT,
            selectcolor=PANEL_2,
            bd=0,
            highlightthickness=0,
            font=("Segoe UI", 8),
        ).pack(side="left")

        tk.Label(
            console,
            text="Command key/provider command · natural language → Cortex · !command → project shell · /help",
            bg=PANEL,
            fg=MUTED,
            font=("Segoe UI", 8),
            anchor="w",
        ).pack(side="bottom", fill="x", padx=10, pady=(0, 4))

        console_body = tk.Frame(console, bg="#07090b")
        console_body.pack(fill="both", expand=True, padx=8, pady=(0, 5))
        self.console_text = tk.Text(
            console_body,
            bg="#07090b",
            fg=TEXT,
            insertbackground=TEXT,
            selectbackground="#21404a",
            selectforeground=TEXT,
            bd=0,
            relief="flat",
            font=("Consolas", 9),
            wrap="word",
        )
        cscroll = self._dark_scrollbar(console_body, orient="vertical", command=self.console_text.yview)
        self.console_text.configure(yscrollcommand=cscroll.set)
        self.console_text.pack(side="left", fill="both", expand=True)
        cscroll.pack(side="right", fill="y")
        self._configure_log_tags(self.console_text)

        panes.add(nav, minsize=138, width=158)
        panes.add(center, minsize=330, width=430)
        panes.add(console, minsize=460, width=610)
        self.window.after(160, self._set_workspace_sashes)

        statusbar = tk.Frame(parent, bg="#07090b", height=25, highlightthickness=1, highlightbackground="#20262d")
        statusbar.pack(fill="x", side="bottom")
        statusbar.pack_propagate(False)
        self.footer = tk.Label(
            statusbar,
            text="[Status:Loading]",
            bg="#07090b",
            fg=CYAN,
            font=("Consolas", 8),
            anchor="w",
        )
        self.footer.pack(fill="both", padx=10)

    def _build_vault_tab(self, parent: Any) -> None:
        """Task-oriented Vault workspace; heavy views are not stacked vertically."""
        tk = self.tk
        shell = tk.Frame(parent, bg=BG)
        shell.pack(fill="both", expand=True, padx=16, pady=12)
        self._section_title(
            shell, "Vault / Local Forge",
            "Inventory, catalog, intake and storage are separate workflows. Scans never start automatically.",
        )
        nav = tk.Frame(shell, bg=BG)
        nav.pack(fill="x", pady=(0, 8))
        self._vault_sections: dict[str, Any] = {}
        self._vault_tab_buttons: dict[str, Any] = {}
        self._active_vault_section = ""
        for title in ("Inventory", "Catalog", "Intake", "Storage", "Health"):
            button = self._button(nav, title, lambda name=title: self._show_vault_section(name), compact=True)
            button.pack(side="left", padx=(0, 6))
            self._vault_tab_buttons[title] = button
            self._vault_sections[title] = tk.Frame(shell, bg=BG)

        shared_status = tk.Frame(shell, bg=PANEL, highlightthickness=1, highlightbackground=BORDER)
        shared_status.pack(fill="x", pady=(0, 9))
        tk.Label(shared_status, text="VAULT ACTIVITY", bg=PANEL, fg=MUTED,
                 font=("Segoe UI Semibold", 8)).pack(side="left", padx=(10, 8), pady=3)
        self.vault_scan_status = tk.Label(shared_status, text="Idle", bg=PANEL, fg=MUTED,
                                          anchor="w", font=("Segoe UI", 9))
        self.vault_scan_status.pack(side="left", fill="x", expand=True, padx=(0, 10))

        self._build_volume_inventory_section(self._vault_sections["Inventory"])
        self._build_vault_catalog_section(self._vault_sections["Catalog"])
        self._build_vault_intake_section(self._vault_sections["Intake"])
        self._build_vault_storage_section(self._vault_sections["Storage"])
        self._build_vault_health_section(self._vault_sections["Health"])
        self._show_vault_section("Inventory")
        # The catalog tree is lazy; merely opening this app surface does not start a scan.
        self.vault_scan_status.configure(text="Inventory is read-only · catalog is available on its own tab", fg=MUTED)

    def _show_vault_section(self, name: str) -> None:
        for title, frame in self._vault_sections.items():
            frame.pack_forget()
            selected = title == name
            self._vault_tab_buttons[title].configure(
                fg=CYAN if selected else TEXT,
                font=("Segoe UI Semibold" if selected else "Segoe UI", 10),
                relief="flat",
            )
        self._vault_sections[name].pack(fill="both", expand=True)
        self._active_vault_section = name

    def _build_volume_inventory_section(self, parent: Any) -> None:
        tk = self.tk
        ttk = self.ttk
        heading = tk.Frame(parent, bg=BG)
        heading.pack(fill="x", pady=(1, 3))
        tk.Label(heading, text="External drive inventory", bg=BG, fg=TEXT,
                 font=("Segoe UI Semibold", 14)).pack(side="left")
        tk.Label(heading, text="READ ONLY · METADATA", bg=PANEL_2, fg=CYAN,
                 font=("Segoe UI Semibold", 9), padx=10, pady=5).pack(side="right")

        self.volume_root_label = tk.Label(parent, text="Resolving portable volume…",
                                          bg=BG, fg=MUTED, anchor="w", font=("Segoe UI", 10))
        self.volume_root_label.pack(fill="x", pady=(0, 3))

        progress_shell = tk.Frame(parent, bg=PANEL, highlightthickness=1, highlightbackground=BORDER)
        progress_shell.pack(fill="x", pady=(0, 5))
        progress_head = tk.Frame(progress_shell, bg=PANEL)
        progress_head.pack(fill="x", padx=12, pady=(6, 2))
        self.volume_status = tk.Label(progress_head, text="Not scanned", bg=PANEL, fg=MUTED,
                                      anchor="w", font=("Segoe UI Semibold", 10))
        self.volume_status.pack(side="left", fill="x", expand=True)
        tk.Label(progress_head, text="Persistent index · activity indicator · no entry cap", bg=PANEL, fg=MUTED,
                 font=("Segoe UI", 9)).pack(side="right")
        self.volume_progress = ttk.Progressbar(progress_shell, orient="horizontal", mode="indeterminate")
        self.volume_progress.pack(fill="x", padx=12, pady=(0, 5))
        self.volume_counts = tk.Label(progress_shell,
            text="0 entries · 0 files · 0 folders · 0 links · 0 errors",
            bg=PANEL, fg=MUTED, anchor="w", font=("Segoe UI", 9))
        self.volume_counts.pack(fill="x", padx=12, pady=(0, 5))

        actions = tk.Frame(parent, bg=BG)
        actions.pack(fill="x", pady=(0, 5))
        self.volume_start_btn = self._button(actions, "Start / Resume Index", self._start_volume_inventory,
                                              primary=True, compact=True)
        self.volume_start_btn.pack(side="left", padx=(0, 7))
        self.volume_cancel_btn = self._button(actions, "Pause Scan", self._cancel_volume_inventory,
                                               compact=True)
        self.volume_cancel_btn.pack(side="left", padx=(0, 12))
        self.volume_cancel_btn.configure(state="disabled")
        tk.Label(actions, text="Writes a dedicated Cortex metadata index only · no source/Vault catalog changes",
                 bg=BG, fg=MUTED, font=("Segoe UI", 9), anchor="w").pack(side="left", fill="x", expand=True)

        panes = tk.PanedWindow(parent, orient="horizontal", bg=BG, sashwidth=6,
                               sashrelief="flat", bd=0)
        panes.pack(fill="both", expand=True)
        left = tk.Frame(panes, bg=PANEL, highlightthickness=1, highlightbackground=BORDER)
        right = tk.Frame(panes, bg=PANEL, highlightthickness=1, highlightbackground=BORDER)
        panes.add(left, minsize=380, stretch="always")
        panes.add(right, minsize=280, stretch="always")

        tk.Label(left, text="RESULTS", bg=PANEL, fg=CYAN,
                 font=("Segoe UI Semibold", 10)).pack(anchor="w", padx=12, pady=(11, 5))
        filter_row = tk.Frame(left, bg=PANEL)
        filter_row.pack(fill="x", padx=12, pady=(0, 6))
        self.volume_query_var = tk.StringVar()
        volume_search = tk.Entry(filter_row, textvariable=self.volume_query_var,
                                  bg="#090c10", fg=TEXT, insertbackground=TEXT,
                                  relief="flat", font=("Segoe UI", 10))
        volume_search.pack(side="left", fill="x", expand=True, ipady=6)
        volume_search.bind("<Return>", lambda _e: self._volume_apply_quick_filter(self.volume_query_var.get()))
        self._button(filter_row, "Filter",
                     lambda: self._volume_apply_quick_filter(self.volume_query_var.get()),
                     compact=True).pack(side="left", padx=(6, 0))
        shortcuts = tk.Frame(left, bg=PANEL)
        shortcuts.pack(fill="x", padx=12, pady=(0, 3))
        for label, needle in (("Top level", ""), ("Projects", "projects/"),
                              ("Git", "Git/"), ("Cortex", "Cortex")):
            self._button(shortcuts, label,
                         lambda value=needle: self._volume_apply_quick_filter(value), compact=True).pack(
                             side="left", padx=(0, 6))
        self.volume_results_status = tk.Label(left, text="Run an inventory to inspect paths.",
                                               bg=PANEL, fg=MUTED, anchor="w",
                                               font=("Segoe UI", 9))
        self.volume_results_status.pack(fill="x", padx=12, pady=(0, 2))
        paging = tk.Frame(left, bg=PANEL)
        paging.pack(fill="x", padx=12, pady=(0, 4))
        self.volume_prev_btn = self._button(paging, "Previous", lambda: self._volume_change_page(-1), compact=True)
        self.volume_prev_btn.pack(side="left", padx=(0, 6))
        self.volume_next_btn = self._button(paging, "Next", lambda: self._volume_change_page(1), compact=True)
        self.volume_next_btn.pack(side="left", padx=(0, 8))
        self.volume_page_label = tk.Label(paging, text="Page 1", bg=PANEL, fg=MUTED, font=("Segoe UI", 9))
        self.volume_page_label.pack(side="left")
        self.volume_prev_btn.configure(state="disabled")
        self.volume_next_btn.configure(state="disabled")
        tree_host = tk.Frame(left, bg=PANEL)
        tree_host.pack(fill="both", expand=True, padx=12, pady=(0, 6))
        self.volume_tree = ttk.Treeview(tree_host, columns=("path", "kind", "size"),
                                         show="headings", selectmode="browse")
        for col, title, width, stretch in (("path", "Relative path", 410, True),
                                           ("kind", "Type", 90, False),
                                           ("size", "Size", 90, False)):
            self.volume_tree.heading(col, text=title)
            self.volume_tree.column(col, width=width, anchor="w", stretch=stretch)
        vscroll = self._dark_scrollbar(tree_host, orient="vertical", command=self.volume_tree.yview)
        hscroll = self._dark_scrollbar(tree_host, orient="horizontal", command=self.volume_tree.xview)
        self.volume_tree.configure(yscrollcommand=vscroll.set, xscrollcommand=hscroll.set)
        hscroll.pack(side="bottom", fill="x")
        vscroll.pack(side="right", fill="y")
        self.volume_tree.pack(side="left", fill="both", expand=True)
        self.volume_tree.bind("<<TreeviewSelect>>", self._volume_selected)
        self._volume_rows: dict[str, dict[str, Any]] = {}

        tk.Label(right, text="ITEM DETAILS", bg=PANEL, fg=CYAN,
                 font=("Segoe UI Semibold", 10)).pack(anchor="w", padx=12, pady=(11, 7))
        selection_actions = tk.Frame(right, bg=PANEL)
        selection_actions.pack(fill="x", padx=12, pady=(0, 7))
        self._button(selection_actions, "Copy Path", self._volume_copy_selected_path,
                     compact=True).pack(side="left", padx=(0, 6))
        self._button(selection_actions, "Reveal", self._volume_reveal_selected,
                     compact=True).pack(side="left", padx=(0, 6))
        self._button(selection_actions, "Access Gaps", self._volume_show_errors,
                     compact=True).pack(side="left")
        body = tk.Frame(right, bg="#07090b", highlightthickness=1, highlightbackground=BORDER)
        body.pack(fill="both", expand=True, padx=12, pady=(0, 12))
        self.volume_detail = tk.Text(body, height=12, wrap="word", bg="#07090b", fg=TEXT,
                                      insertbackground=TEXT, bd=0, relief="flat",
                                      font=("Consolas", 10))
        dscroll = self._dark_scrollbar(body, orient="vertical", command=self.volume_detail.yview)
        self.volume_detail.configure(yscrollcommand=dscroll.set)
        self.volume_detail.pack(side="left", fill="both", expand=True, padx=(8, 0), pady=8)
        dscroll.pack(side="right", fill="y", padx=(4, 6), pady=6)
        self._volume_set_detail("Filesystem discovery is read-only. Its SQLite metadata index lives under "
                                "Cortex state, separate from vault_catalog.db.\n"
                                "Pause or close retains committed inventory batches.")
        self.window.after(150, self._volume_load_existing)

    def _build_vault_catalog_section(self, parent: Any) -> None:
        tk = self.tk
        ttk = self.ttk
        toolbar = tk.Frame(parent, bg=BG)
        toolbar.pack(fill="x", pady=(0, 9))
        tk.Label(toolbar, text="Project catalog & browser", bg=BG, fg=TEXT,
                 font=("Segoe UI Semibold", 13)).pack(anchor="w", pady=(0, 5))
        self._toolbar_grid(toolbar, (
            ("Scan Active Project", lambda: self._start_vault_scan(False), True, False),
            ("Deep Hash Scan", lambda: self._start_vault_scan(True), False, False),
            ("Refresh Browser", self._vault_refresh_tree, False, False),
            ("Open Catalog", lambda: open_path(vault_catalog_dir(self.root_path)), False, False),
            ("Open Vault Root", lambda: open_path(global_vault_root()), False, False),
            ("Capture Baseline", self._vault_capture_baseline, False, False),
            ("Compare Baseline", self._vault_compare_baseline, False, False),
        ), preferred_columns=4, minimum_cell_width=145)

        metrics = tk.Frame(parent, bg=BG)
        metrics.pack(fill="x", pady=(0, 9))
        self.vault_metric_labels: dict[str, Any] = {}
        for key, title in (
            ("files", "Cataloged"),
            ("assets", "Assets"),
            ("large", "Large Files"),
            ("duplicates", "Duplicate Groups"),
            ("json", "Invalid JSON"),
            ("excluded", "Pruned Dirs"),
        ):
            card = tk.Frame(metrics, bg=PANEL, highlightthickness=1, highlightbackground=BORDER)
            card.pack(side="left", fill="x", expand=True, padx=(0, 8))
            tk.Label(card, text=title, bg=PANEL, fg=MUTED, font=("Segoe UI", 8)).pack(anchor="w", padx=12, pady=(8, 1))
            value = tk.Label(card, text="—", bg=PANEL, fg=TEXT, font=("Segoe UI Semibold", 12))
            value.pack(anchor="w", padx=12, pady=(0, 8))
            self.vault_metric_labels[key] = value

        panes = tk.PanedWindow(parent, orient="horizontal", bg=BG, sashwidth=5, sashrelief="flat", bd=0)
        panes.pack(fill="both", expand=True)

        left = self._panel(panes, "Project Files")
        right = self._panel(panes, "Catalog Detail")
        panes.add(left, minsize=470, stretch="always")
        panes.add(right, minsize=390, stretch="always")

        search_row = tk.Frame(left, bg=PANEL)
        search_row.pack(fill="x", padx=10, pady=(0, 7))
        self.vault_search_var = tk.StringVar()
        search = tk.Entry(
            search_row,
            textvariable=self.vault_search_var,
            bg="#090c10",
            fg=TEXT,
            insertbackground=TEXT,
            relief="flat",
            font=("Segoe UI", 9),
        )
        search.pack(side="left", fill="x", expand=True, ipady=6)
        search.bind("<Return>", lambda _e: self._vault_search())
        self._button(search_row, "Search Catalog", self._vault_search, compact=True).pack(side="left", padx=(6, 0))
        self._button(search_row, "Clear", self._vault_clear_search, compact=True).pack(side="left", padx=(6, 0))

        tree_shell = tk.Frame(left, bg=PANEL)
        tree_shell.pack(fill="both", expand=True, padx=10, pady=(0, 10))
        self.vault_tree = ttk.Treeview(
            tree_shell,
            columns=("class", "size", "modified"),
            show="tree headings",
            selectmode="browse",
        )
        self.vault_tree.heading("#0", text="Name")
        self.vault_tree.column("#0", width=360, anchor="w")
        for key, title, width in (("class", "Class", 130), ("size", "Size", 100), ("modified", "Modified", 150)):
            self.vault_tree.heading(key, text=title)
            self.vault_tree.column(key, width=width, anchor="w")
        self.vault_tree.tag_configure("SOURCE", foreground=GREEN)
        self.vault_tree.tag_configure("ASSET", foreground=CYAN)
        self.vault_tree.tag_configure("BUILD_OUTPUT", foreground=MUTED)
        self.vault_tree.tag_configure("DEPENDENCY", foreground=MUTED)
        self.vault_tree.tag_configure("ARCHIVE", foreground=YELLOW)
        self.vault_tree.tag_configure("CONTROL", foreground="#d29dff")
        vscroll = self._dark_scrollbar(tree_shell, orient="vertical", command=self.vault_tree.yview)
        self.vault_tree.configure(yscrollcommand=vscroll.set)
        self.vault_tree.pack(side="left", fill="both", expand=True)
        vscroll.pack(side="right", fill="y")
        self.vault_tree.bind("<<TreeviewOpen>>", self._vault_tree_opened)
        self.vault_tree.bind("<<TreeviewSelect>>", self._vault_tree_selected)
        self.vault_tree.bind("<Double-1>", lambda _e: self._vault_open_selected())

        right_actions = tk.Frame(right, bg=PANEL)
        right_actions.pack(fill="x", padx=12, pady=(0, 8))
        self._button(right_actions, "Open", self._vault_open_selected, primary=True, compact=True).pack(side="left", padx=(0, 6))
        self._button(right_actions, "Reveal", self._vault_reveal_selected, compact=True).pack(side="left", padx=6)
        self._button(right_actions, "Copy Path", self._vault_copy_selected_path, compact=True).pack(side="left", padx=6)

        detail_shell = tk.Frame(right, bg="#07090b", highlightthickness=1, highlightbackground=BORDER)
        detail_shell.pack(fill="both", expand=True, padx=12, pady=(0, 12))
        self.vault_detail = tk.Text(
            detail_shell,
            bg="#07090b",
            fg=TEXT,
            insertbackground=TEXT,
            bd=0,
            relief="flat",
            font=("Consolas", 9),
            wrap="word",
            height=10,
        )
        dscroll = self._dark_scrollbar(detail_shell, orient="vertical", command=self.vault_detail.yview)
        self.vault_detail.configure(yscrollcommand=dscroll.set)
        self.vault_detail.pack(side="left", fill="both", expand=True, padx=(10, 0), pady=10)
        dscroll.pack(side="right", fill="y", padx=(4, 8), pady=8)
        self.vault_detail.configure(state="disabled")

    def _build_vault_intake_section(self, parent: Any) -> None:
        tk = self.tk
        tk.Label(parent, text="Intake & classification", bg=BG, fg=TEXT,
                 font=("Segoe UI Semibold", 13)).pack(anchor="w", pady=(0, 8))
        self._toolbar_grid(parent, (
            ("Audit Intake", self._start_vault_intake_audit, True, False),
            ("Create Chat Handoff", self._start_vault_intake_handoff, False, False),
            ("Open Intake Audit", lambda: open_path(vault_intake_audit_dir(self.root_path)), False, False),
            ("Open Intake Folder", lambda: open_path(vault_intake_root(self.root_path)), False, False),
            ("Stop Vault Scan", self._vault_stop_scan, False, False),
        ), preferred_columns=3, minimum_cell_width=150)
        tk.Label(parent,
            text="Audit is non-destructive: no move, rename, delete, execute or archive extraction. "
                 "Review the handoff before any organization.", bg=BG, fg=MUTED,
            anchor="w", justify="left", wraplength=800, font=("Segoe UI", 10)).pack(fill="x", pady=(10, 0))

    def _build_vault_storage_section(self, parent: Any) -> None:
        tk = self.tk
        tk.Label(parent, text="Storage authority", bg=BG, fg=TEXT,
                 font=("Segoe UI Semibold", 13)).pack(anchor="w", pady=(0, 8))
        self._toolbar_grid(parent, (
            ("Mirror Project", self._start_vault_mirror, True, False),
            ("Mirror All Projects", self._start_vault_mirror_all, False, False),
            ("Verify Mirror", self._vault_verify_mirror, False, False),
            ("Shared Dependencies", self._vault_dependency_info, False, False),
            ("Space Reclaim Plan", self._vault_reclaim_plan, False, False),
            ("Open Project Mirror", lambda: open_path(vault_project_dir(self.root_path)), False, False),
        ), preferred_columns=3, minimum_cell_width=150)

    def _build_vault_health_section(self, parent: Any) -> None:
        tk = self.tk
        tk.Label(parent, text="Health & lifecycle", bg=BG, fg=TEXT,
                 font=("Segoe UI Semibold", 13)).pack(anchor="w", pady=(0, 8))
        self._toolbar_grid(parent, (
            ("Storage Health", self._vault_storage_health, True, False),
            ("Retention Plan", self._vault_retention_plan, False, False),
            ("CAS GC Plan", self._vault_gc_plan, False, False),
        ), preferred_columns=3, minimum_cell_width=150)
        tk.Label(parent, text="Mutating maintenance remains CLI + --yes until Windows certification.",
                 bg=BG, fg=MUTED, anchor="w", font=("Segoe UI", 10)).pack(fill="x", pady=(9, 0))

    def _volume_set_detail(self, text: str) -> None:
        self.volume_detail.configure(state="normal")
        self.volume_detail.delete("1.0", "end")
        self.volume_detail.insert("1.0", text)
        self.volume_detail.configure(state="disabled")

    def _volume_apply_quick_filter(self, value: str) -> None:
        self.volume_query_var.set(value)
        self._volume_page = 0
        self._show_volume_inventory()

    def _volume_change_page(self, delta: int) -> None:
        self._volume_page = max(0, self._volume_page + delta)
        self._show_volume_inventory()

    def _volume_load_existing(self) -> None:
        """Discover an index without starting a scan or creating a database."""
        try:
            mount, database, volume_id = persistent_volume_location(self.root_path)
            self._volume_root, self._volume_db_path, self._volume_id = mount, database, volume_id
            self.volume_root_label.configure(text=f"Development volume: {mount} · ID: {volume_id}")
        except Exception as exc:
            self.volume_status.configure(text=f"Portable volume unavailable: {exc}", fg=YELLOW)
            self.volume_start_btn.configure(state="disabled")
            return

        def work() -> None:
            try:
                result = persistent_volume_status(database, mount, volume_id=volume_id)
                self._event_q.put(("volume-index-loaded", result))
            except Exception as exc:
                self._event_q.put(("volume-error", f"Existing index could not be opened: {exc}"))
        threading.Thread(target=work, daemon=True).start()

    def _volume_refresh_status(self, report: dict[str, Any]) -> None:
        self._volume_result = report
        state = str(report.get("state", "not_started"))
        count = int(report.get("entries") or 0)
        errors = int(report.get("errors") or 0)
        pending = int(report.get("pending_directories") or 0)
        counts = report.get("counts") or {}
        display = ("Interrupted / resumable" if state == "running" and not self._volume_running else
                   "SCANNING" if state == "running" else
                   "PAUSED / RESUMABLE" if state == "paused" else
                   "COMPLETE WITH ACCESS GAPS" if state == "complete" and errors else
                   "COMPLETE" if state == "complete" else "Not indexed")
        self.volume_status.configure(text=f"{display} · {count:,} indexed", fg=CYAN if self._volume_running else
                                     YELLOW if pending or errors else GREEN if state == "complete" else MUTED)
        self.volume_counts.configure(
            text=f"{count:,} entries · {int(counts.get('file') or 0):,} files · "
                 f"{int(counts.get('directory') or 0):,} folders · "
                 f"{int(counts.get('symlink') or 0):,} links · {errors:,} errors · "
                 f"{pending:,} directories pending")
        if state == "complete":
            self.volume_start_btn.configure(text="Rebuild Index")
        else:
            self.volume_start_btn.configure(text="Start / Resume Index")

    def _volume_selected_path(self) -> Path | None:
        selected = self.volume_tree.selection()
        if not selected or self._volume_root is None:
            return None
        entry = self._volume_rows.get(selected[0])
        if entry is None:
            return None
        # All database paths were obtained from scandir and stored volume-relative.
        return self._volume_root.joinpath(*str(entry["path"]).split("/"))

    def _volume_selected(self, _event: Any = None) -> None:
        selected = self.volume_tree.selection()
        if not selected:
            return
        entry = self._volume_rows.get(selected[0])
        if entry is None:
            return
        path = self._volume_selected_path()
        stamp = entry.get("modified_ns")
        modified = (datetime.fromtimestamp(int(stamp) / 1_000_000_000).isoformat(sep=" ", timespec="seconds")
                    if stamp is not None else "Unknown")
        size = (self._human_bytes(int(entry["size_bytes"])) if entry.get("size_bytes") is not None else "—")
        self._volume_set_detail(
            f"Relative path: {entry['path']}\n\nFull path: {path}\n\n"
            f"Type: {entry.get('type', 'other')}\nSize: {size}\nModified: {modified}\n"
            f"Boundary: {entry.get('boundary', 'none')}\n\n"
            "Discovery metadata only. No source files moved, opened, imported or registered."
        )

    def _volume_copy_selected_path(self) -> None:
        path = self._volume_selected_path()
        if path is None:
            return
        self.window.clipboard_clear()
        self.window.clipboard_append(str(path))
        self.volume_results_status.configure(text="Selected path copied", fg=GREEN)

    def _volume_reveal_selected(self) -> None:
        path = self._volume_selected_path()
        if path is not None and path.exists():
            reveal_file(path)

    def _volume_show_errors(self) -> None:
        if not self._volume_db_path or not self._volume_id or not self._volume_db_path.is_file():
            return
        db_path, volume_id = self._volume_db_path, self._volume_id
        def work() -> None:
            try:
                rows = persistent_volume_errors(db_path, volume_id=volume_id, limit=30)
                self._event_q.put(("volume-errors-done", rows))
            except Exception as exc:
                self._event_q.put(("volume-errors-error", str(exc)))
        threading.Thread(target=work, daemon=True).start()

    @staticmethod
    def _human_bytes(value: int) -> str:
        amount = float(max(0, int(value)))
        units = ["B", "KB", "MB", "GB", "TB"]
        for unit in units:
            if amount < 1024.0 or unit == units[-1]:
                return f"{amount:.0f} {unit}" if unit == "B" else f"{amount:.1f} {unit}"
            amount /= 1024.0
        return f"{int(value)} B"

    def _vault_refresh_tree(self) -> None:
        if not hasattr(self, "vault_tree"):
            return
        for iid in self.vault_tree.get_children():
            self.vault_tree.delete(iid)
        self._vault_node_paths.clear()
        root = self.root_path
        iid = "vault-root"
        self.vault_tree.insert("", "end", iid=iid, text=root.name, values=("PRIMARY_PROJECT", "", ""), open=True, tags=("SOURCE",))
        self._vault_node_paths[iid] = root
        self._vault_insert_children(iid, root)
        self._vault_render_summary(vault_latest_summary(root))

    def _vault_insert_children(self, parent_iid: str, path: Path) -> None:
        try:
            entries = sorted(path.iterdir(), key=lambda p: (not p.is_dir(), p.name.casefold()))
        except OSError:
            return
        # Remove lazy placeholder if present.
        for child in self.vault_tree.get_children(parent_iid):
            if str(child).startswith("dummy:"):
                self.vault_tree.delete(child)
        for child in entries:
            try:
                is_dir = child.is_dir()
                stat = child.stat()
            except OSError:
                continue
            classification = vault_classify_path(self.root_path, child, is_dir=is_dir)
            rel = child.relative_to(self.root_path).as_posix()
            iid = "vault:" + rel.replace("/", "\\")
            if self.vault_tree.exists(iid):
                continue
            size = "" if is_dir else self._human_bytes(int(stat.st_size))
            modified = datetime.fromtimestamp(stat.st_mtime).strftime("%Y-%m-%d %H:%M")
            self.vault_tree.insert(parent_iid, "end", iid=iid, text=child.name, values=(classification, size, modified), tags=(classification,))
            self._vault_node_paths[iid] = child
            if is_dir:
                try:
                    next(child.iterdir())
                    dummy = "dummy:" + iid
                    self.vault_tree.insert(iid, "end", iid=dummy, text="…")
                except (StopIteration, OSError):
                    pass

    def _vault_tree_opened(self, _event: Any = None) -> None:
        iid = self.vault_tree.focus()
        path = self._vault_node_paths.get(iid)
        if path and path.is_dir():
            self._vault_insert_children(iid, path)

    def _vault_tree_selected(self, _event: Any = None) -> None:
        iid = self.vault_tree.focus()
        path = self._vault_node_paths.get(iid)
        if not path:
            return
        self._vault_show_path(path)

    def _vault_show_path(self, path: Path) -> None:
        try:
            rel = path.relative_to(self.root_path).as_posix() if path != self.root_path else "."
        except ValueError:
            rel = str(path)
        try:
            stat = path.stat()
            size = "Directory" if path.is_dir() else self._human_bytes(stat.st_size)
            modified = datetime.fromtimestamp(stat.st_mtime).strftime("%Y-%m-%d %H:%M:%S")
        except OSError:
            size, modified = "Unavailable", "Unavailable"
        classification = "PRIMARY_PROJECT" if path == self.root_path else vault_classify_path(self.root_path, path, is_dir=path.is_dir())
        rec = None if path.is_dir() else vault_catalog_record(self.root_path, rel)
        lines = [
            f"Path           : {path}",
            f"Relative       : {rel}",
            f"Classification : {classification}",
            f"Size           : {size}",
            f"Modified       : {modified}",
        ]
        if rec:
            lines.extend([
                f"Catalog SHA256 : {rec.get('sha256') or '<deferred>'}",
                f"JSON valid     : {rec.get('jsonValid') if rec.get('jsonValid') is not None else 'n/a'}",
                f"Catalog note   : {rec.get('note') or '—'}",
            ])
        else:
            lines.append("Catalog        : Run Scan Active Project to index/hash this item.")
        self.vault_detail.configure(state="normal")
        self.vault_detail.delete("1.0", "end")
        self.vault_detail.insert("1.0", "\n".join(lines))
        self.vault_detail.configure(state="disabled")

    def _vault_selected_path(self) -> Path | None:
        iid = self.vault_tree.focus() if hasattr(self, "vault_tree") else ""
        return self._vault_node_paths.get(iid)

    def _vault_open_selected(self) -> None:
        path = self._vault_selected_path()
        if path:
            open_path(path)

    def _vault_reveal_selected(self) -> None:
        path = self._vault_selected_path()
        if not path:
            return
        if path.is_file():
            reveal_file(path)
        else:
            open_path(path)

    def _vault_copy_selected_path(self) -> None:
        path = self._vault_selected_path()
        if path:
            self._copy_to_clipboard(str(path), "Vault Path")

    def _vault_clear_search(self) -> None:
        if hasattr(self, "vault_search_var"):
            self.vault_search_var.set("")
        self._vault_refresh_tree()

    def _vault_search(self) -> None:
        query = self.vault_search_var.get().strip()
        if not query:
            self._vault_refresh_tree()
            return
        results = vault_search_catalog(self.root_path, query)
        for iid in self.vault_tree.get_children():
            self.vault_tree.delete(iid)
        self._vault_node_paths.clear()
        root_iid = "vault-search"
        self.vault_tree.insert("", "end", iid=root_iid, text=f"Search: {query}", values=("CATALOG_SEARCH", f"{len(results)} result(s)", ""), open=True)
        for index, item in enumerate(results):
            rel = str(item.get("relPath") or "")
            path = self.root_path / Path(rel)
            iid = f"search:{index}"
            self.vault_tree.insert(root_iid, "end", iid=iid, text=rel, values=(item.get("classification") or "", self._human_bytes(int(item.get("bytes") or 0)), ""), tags=(str(item.get("classification") or ""),))
            self._vault_node_paths[iid] = path

    def _start_volume_inventory(self) -> None:
        if self._vault_busy:
            self._popup("Volume Inventory", "Another Vault operation is running.", kind="warning")
            return
        if not self._volume_root or not self._volume_db_path or not self._volume_id:
            self._popup("Volume Inventory", "A marked portable Cortex volume is required.", kind="error")
            return
        if not self._volume_root.is_dir():
            self._popup("Volume Inventory", f"Drive root is unavailable: {self._volume_root}", kind="error")
            return
        restart = bool(self._volume_result and self._volume_result.get("state") == "complete")
        if restart and not self._popup("Rebuild Inventory Index",
                "The inventory is already complete. Rebuild its dedicated SQLite metadata index?\n\n"
                "Only the inventory index is replaced. Source files and vault_catalog.db are untouched.",
                kind="warning", confirm=True):
            return
        self._vault_busy = True
        self._volume_running = True
        self._vault_cancel = False
        self.volume_start_btn.configure(state="disabled")
        self.volume_cancel_btn.configure(state="normal")
        self.volume_progress.start(18)
        self.volume_status.configure(text=f"Scanning {self._volume_root} · committed batches are searchable", fg=CYAN)
        self._append_log(f"[INFO] Explicit persistent volume inventory {'rebuild' if restart else 'start/resume'}: "
                         f"{self._volume_root} · index: {self._volume_db_path}\n", "info")
        volume_root, db_path, volume_id = self._volume_root, self._volume_db_path, self._volume_id
        last_emit = [0.0]

        def progress(payload: dict[str, Any]) -> None:
            now = time.monotonic()
            if now - last_emit[0] >= .3 or payload.get("state") != "running":
                last_emit[0] = now
                self._event_q.put(("volume-progress", payload))

        def work() -> None:
            try:
                report = persistent_volume_scan(volume_root, db_path, volume_id=volume_id,
                    progress=progress, cancelled=lambda: self._vault_cancel, restart=restart)
                self._event_q.put(("volume-done", report))
            except Exception as exc:
                self._event_q.put(("volume-error", str(exc)))

        threading.Thread(target=work, daemon=True).start()

    def _cancel_volume_inventory(self) -> None:
        if not self._volume_running:
            self.volume_status.configure(text="No active drive scan", fg=MUTED)
            return
        self._vault_cancel = True
        self.volume_cancel_btn.configure(state="disabled")
        self.volume_status.configure(text="Pause requested · committing partial inventory…", fg=YELLOW)

    def _show_volume_inventory(self) -> None:
        if not self._volume_db_path or not self._volume_id or not self._volume_root:
            self.volume_results_status.configure(text="Portable volume unavailable.", fg=MUTED)
            return
        if not self._volume_db_path.is_file():
            self.volume_results_status.configure(text="No index yet. Start a scan to populate persistent results.", fg=MUTED)
            return
        query = self.volume_query_var.get()
        self._volume_view_generation += 1
        if self._volume_filter_busy:
            # Coalesce repeated button presses and live refreshes; never flood SQLite
            # with one full-index COUNT query per keystroke/action.
            self._volume_filter_pending = True
            return
        self._volume_filter_busy = True
        generation = self._volume_view_generation
        page = self._volume_page
        db_path, root, volume_id = self._volume_db_path, self._volume_root, self._volume_id
        self.volume_results_status.configure(text="Loading a database page in background…", fg=CYAN)

        def work() -> None:
            try:
                view = persistent_volume_query(db_path, volume_id=volume_id,
                    query=query, page=page, page_size=self._volume_page_size)
                status = persistent_volume_status(db_path, root, volume_id=volume_id)
                self._event_q.put(("volume-view-done", (generation, view, status)))
            except Exception as exc:
                self._event_q.put(("volume-view-error", (generation, str(exc))))

        threading.Thread(target=work, daemon=True).start()

    def _start_vault_scan(self, deep: bool) -> None:
        if self._vault_busy:
            self._popup("Vault Scan", "A Vault/Forge scan is already running.", kind="warning")
            return
        self._vault_busy = True
        self._vault_cancel = False
        self.vault_scan_status.configure(text="Deep hash scan…" if deep else "Scanning…", fg=CYAN)
        root = self.root_path

        def progress(payload: dict[str, Any]) -> None:
            self._event_q.put(("vault-progress", payload))

        def work() -> None:
            try:
                summary = vault_scan_project(root, deep_hash=deep, progress=progress, cancelled=lambda: self._vault_cancel)
                self._event_q.put(("vault-done", (root, summary)))
            except Exception as exc:
                self._event_q.put(("vault-error", str(exc)))

        threading.Thread(target=work, daemon=True).start()

    def _vault_stop_scan(self) -> None:
        if not self._vault_busy:
            self.vault_scan_status.configure(text="Idle", fg=MUTED)
            return
        self._vault_cancel = True
        self.vault_scan_status.configure(text="Cancellation requested…", fg=YELLOW)
        self._append_log("[INFO] Vault scan cancellation requested by operator.\n", "info")

    def _start_vault_intake_audit(self) -> None:
        if self._vault_busy:
            self._popup("Vault Intake Audit", "A Vault operation is already running.", kind="warning")
            return
        self._vault_busy = True
        self._vault_cancel = False
        self.vault_scan_status.configure(text="Auditing Intake…", fg=CYAN)
        root = self.root_path

        def progress(payload: dict[str, Any]) -> None:
            self._event_q.put(("vault-intake-progress", payload))

        def work() -> None:
            try:
                summary = vault_intake_scan(
                    root,
                    progress=progress,
                    cancelled=lambda: self._vault_cancel,
                )
                self._event_q.put(("vault-intake-done", summary))
            except InterruptedError as exc:
                self._event_q.put(("vault-intake-cancelled", str(exc)))
            except Exception as exc:
                self._event_q.put(("vault-intake-error", str(exc)))

        threading.Thread(target=work, daemon=True).start()

    def _start_vault_intake_handoff(self) -> None:
        if self._vault_busy:
            self._popup("Vault Intake Handoff", "A Vault operation is already running.", kind="warning")
            return
        if not vault_intake_latest_summary(self.root_path):
            self._popup("Vault Intake Handoff", "No Intake audit exists yet. Run Audit Intake first.", kind="warning")
            return
        self._vault_busy = True
        self.vault_scan_status.configure(text="Creating Intake handoff…", fg=CYAN)
        root = self.root_path

        def work() -> None:
            try:
                path = vault_intake_create_handoff(root)
                self._event_q.put(("vault-intake-handoff-done", str(path)))
            except Exception as exc:
                self._event_q.put(("vault-intake-error", str(exc)))

        threading.Thread(target=work, daemon=True).start()

    def _start_vault_mirror(self) -> None:
        if self._vault_busy:
            self._popup("Vault Mirror", "A Vault operation is already running.", kind="warning")
            return
        self._vault_busy = True
        self.vault_scan_status.configure(text="Mirroring project…", fg=CYAN)
        root = self.root_path

        def progress(payload: dict[str, Any]) -> None:
            self._event_q.put(("vault-mirror-progress", payload))

        def work() -> None:
            try:
                result = vault_mirror_project(root, label="gui-working", progress=progress)
                self._event_q.put(("vault-mirror-done", (root, result)))
            except Exception as exc:
                self._event_q.put(("vault-mirror-error", str(exc)))

        threading.Thread(target=work, daemon=True).start()

    def _start_vault_mirror_all(self) -> None:
        if self._vault_busy:
            self._popup("Vault Mirror", "A Vault operation is already running.", kind="warning")
            return
        self._vault_busy = True
        self.vault_scan_status.configure(text="Mirroring registered projects…", fg=CYAN)
        try:
            entries = self.registry.entries()
        except Exception as exc:
            self._vault_busy = False
            self._popup("Vault Mirror", str(exc), kind="error")
            return
        roots: list[Path] = []
        seen: set[str] = set()
        for entry in entries:
            root = Path(entry.root).expanduser().resolve()
            key = os.path.normcase(str(root))
            if root.is_dir() and key not in seen:
                seen.add(key)
                roots.append(root)
        current = self.root_path.resolve()
        if os.path.normcase(str(current)) not in seen:
            roots.insert(0, current)

        def work() -> None:
            results: list[dict[str, Any]] = []
            failures: list[str] = []
            for index, root in enumerate(roots, start=1):
                self._event_q.put(("vault-mirror-all-progress", {"index": index, "total": len(roots), "root": str(root)}))
                try:
                    results.append(vault_mirror_project(root, label="registered-project-sweep"))
                except Exception as exc:
                    failures.append(f"{root}: {exc}")
            self._event_q.put(("vault-mirror-all-done", {"total": len(roots), "results": results, "failures": failures}))

        threading.Thread(target=work, daemon=True).start()

    def _vault_verify_mirror(self) -> None:
        try:
            result = vault_verify_latest_mirror(self.root_path, deep_hash=False)
        except Exception as exc:
            self._popup("Vault Mirror", str(exc), kind="error")
            return
        status = str(result.get("status") or "FAIL")
        message = (
            f"Snapshot: {result.get('snapshotId') or '—'}\n"
            f"Objects checked: {result.get('checkedObjects', 0)}\n"
            f"Missing: {len(result.get('missing') or [])}\n"
            f"Corrupt: {len(result.get('corrupt') or [])}\n\n"
            f"Project mirror: {result.get('projectVault') or vault_project_dir(self.root_path)}"
        )
        self._append_log(f"[{'PASS' if status == 'PASS' else 'FAIL'}] Vault mirror verify: {status}\n", "pass" if status == "PASS" else "fail")
        self._popup("Vault Mirror Verification", message, kind="success" if status == "PASS" else "error")

    def _vault_reclaim_plan(self) -> None:
        try:
            result = vault_reclaim_plan(self.root_path)
        except Exception as exc:
            self._popup("Space Reclaim Plan", str(exc), kind="error")
            return
        reclaim = self._human_bytes(int(result.get("reclaimBytes") or 0))
        candidates = list(result.get("candidates") or [])
        preview = "\n".join(
            f"{item.get('relativePath')} — {self._human_bytes(int(item.get('bytes') or 0))}"
            for item in candidates[:10]
        ) or "No project-local rebuildable caches found."
        message = (
            f"Potential reclaim: {reclaim}\n"
            f"Candidates: {len(candidates)}\n\n"
            f"{preview}\n\n"
            "Dry-run only. Nothing was deleted. A cleanup action should be used only after shared-path Windows certification."
        )
        self._popup("Space Reclaim Plan", message, kind="info")

    def _vault_dependency_info(self) -> None:
        try:
            result = vault_dependency_status(self.root_path)
        except Exception as exc:
            self._popup("Shared Dependencies", str(exc), kind="error")
            return
        paths = result.get("paths") or {}
        message = (
            f"Vault: {result.get('vaultRoot')}\n\n"
            f"Cargo home: {paths.get('cargo_home')}\n"
            f"Cargo target: {paths.get('cargo_target_dir')}\n"
            f"sccache: {result.get('sccache') or 'optional / not installed'}\n"
            f"npm cache: {paths.get('npm_cache')}\n"
            f"pip cache: {paths.get('pip_cache')}\n"
            f"Gradle home: {paths.get('gradle_home')}\n"
            f"NuGet packages: {paths.get('nuget_packages')}\n"
            f"vcpkg downloads: {paths.get('vcpkg_downloads')}"
        )
        self._popup("Shared Dependency Storage", message, kind="info")

    def _vault_storage_health(self) -> None:
        try:
            result = vault_storage_health(self.root_path)
        except Exception as exc:
            self._popup("Storage Health", str(exc), kind="error")
            return
        cas = result.get("cas") or {}
        disk = result.get("disk") or {}
        warnings = list(result.get("warnings") or [])
        message = (
            f"Status: {result.get('status')}\n"
            f"Vault: {result.get('vaultRoot')}\n\n"
            f"Projects: {result.get('projects', 0)}\n"
            f"Snapshots: {result.get('snapshots', 0)}\n"
            f"CAS: {cas.get('files', 0)} objects / {self._human_bytes(int(cas.get('bytes') or 0))}\n"
            f"Vault free: {self._human_bytes(int(disk.get('free') or 0))}\n"
            f"GC eligible: {result.get('gcGarbageObjects', 0)} objects / {self._human_bytes(int(result.get('gcGarbageBytes') or 0))}\n"
            f"Current project reclaim: {self._human_bytes(int(result.get('currentProjectReclaimBytes') or 0))}"
        )
        if warnings:
            message += "\n\nWarnings:\n- " + "\n- ".join(str(x) for x in warnings)
        self._popup("Vault Storage Health", message, kind="warning" if warnings else "success")

    def _vault_retention_plan(self) -> None:
        try:
            result = vault_retention_plan(self.root_path)
        except Exception as exc:
            self._popup("Snapshot Retention", str(exc), kind="error")
            return
        message = (
            f"Snapshots: {result.get('snapshotCount', 0)}\n"
            f"Keep: {result.get('keepCount', 0)}\n"
            f"Eligible metadata prune: {result.get('pruneCount', 0)}\n"
            f"Certified retention: {result.get('certifiedKeep', 0)}\n"
            f"Working retention: {result.get('workingKeep', 0)}\n\n"
            "Dry-run only from the GUI. Retention apply requires explicit CLI --yes."
        )
        self._popup("Vault Snapshot Retention", message, kind="info")

    def _vault_gc_plan(self) -> None:
        try:
            result = vault_cas_gc_plan(self.root_path)
        except Exception as exc:
            self._popup("CAS GC Plan", str(exc), kind="error")
            return
        message = (
            f"CAS objects: {result.get('casObjects', 0)}\n"
            f"Referenced: {result.get('referencedObjects', 0)}\n"
            f"Eligible for quarantine: {result.get('garbageObjects', 0)}\n"
            f"Potential reclaim after purge: {self._human_bytes(int(result.get('garbageBytes') or 0))}\n\n"
            "Plan only. GC first stages objects into reversible Vault quarantine; purge is a separate explicit --yes action."
        )
        self._popup("Vault CAS Garbage Collection", message, kind="info")

    def _vault_capture_baseline(self) -> None:
        try:
            path = vault_capture_baseline(self.root_path)
        except Exception as exc:
            self._popup("Vault Baseline", str(exc), kind="warning")
            return
        self._append_log(f"[PASS] Vault baseline captured: {path}\n", "pass")
        self._popup("Vault Baseline", f"Baseline captured for {self.contract.name}.\n\n{path}", kind="success")

    def _vault_compare_baseline(self) -> None:
        try:
            result = vault_compare_baseline(self.root_path)
        except Exception as exc:
            self._popup("Vault Baseline", str(exc), kind="warning")
            return
        counts = result.get("counts") or {}
        message = (
            f"Added: {counts.get('added', 0)}\n"
            f"Removed: {counts.get('removed', 0)}\n"
            f"Changed: {counts.get('changed', 0)}\n\n"
            f"Report: {vault_catalog_dir(self.root_path) / 'baseline-comparison.json'}"
        )
        self._append_log(f"[INFO] Vault baseline comparison: {counts}\n", "info")
        self._popup("Vault Baseline Comparison", message, kind="info")

    def _vault_render_summary(self, summary: dict[str, Any] | None) -> None:
        if not hasattr(self, "vault_metric_labels"):
            return
        if not summary:
            for label in self.vault_metric_labels.values():
                label.configure(text="—", fg=MUTED)
            return
        classes = summary.get("classCounts") or {}
        values = {
            "files": int(summary.get("files") or 0),
            "assets": int(classes.get("ASSET") or 0),
            "large": int(summary.get("largeFiles") or 0),
            "duplicates": int(summary.get("duplicateGroups") or 0),
            "json": int(summary.get("invalidJson") or 0),
            "excluded": int(summary.get("excludedDirectories") or 0),
        }
        for key, value in values.items():
            color = RED if key == "json" and value else (YELLOW if key in {"large", "duplicates"} and value else GREEN)
            self.vault_metric_labels[key].configure(text=str(value), fg=color)
        self._vault_metrics = summary


    def _panel(self, parent: Any, title: str | None = None) -> Any:
        tk = self.tk
        frame = tk.Frame(parent, bg=BG, bd=0, highlightthickness=0)
        canvas = tk.Canvas(frame, bg=BG, bd=0, highlightthickness=0)
        canvas.place(x=0, y=0, relwidth=1, relheight=1)

        def rounded_rect(c: Any, x1: int, y1: int, x2: int, y2: int, radius: int, **kwargs: Any) -> int:
            r = max(2, min(radius, (x2 - x1) // 2, (y2 - y1) // 2))
            points = [
                x1 + r, y1, x2 - r, y1, x2, y1, x2, y1 + r,
                x2, y2 - r, x2, y2, x2 - r, y2, x1 + r, y2,
                x1, y2, x1, y2 - r, x1, y1 + r, x1, y1,
            ]
            return c.create_polygon(points, smooth=True, splinesteps=18, **kwargs)

        def redraw(_event: Any = None) -> None:
            try:
                w = max(2, frame.winfo_width())
                h = max(2, frame.winfo_height())
                canvas.delete("panel")
                rounded_rect(
                    canvas,
                    1, 1, w - 1, h - 1, 10,
                    fill=PANEL,
                    outline=BORDER,
                    width=1,
                    tags="panel",
                )
            except Exception:
                pass

        frame.bind("<Configure>", redraw, add="+")
        frame._pcc_canvas = canvas  # type: ignore[attr-defined]
        if title:
            tk.Label(
                frame,
                text=title,
                bg=PANEL,
                fg=CYAN,
                font=("Segoe UI Semibold", 10),
            ).pack(anchor="w", padx=13, pady=(11, 5))
        return frame

    def _button(
        self,
        parent: Any,
        text: str,
        command: Callable[[], None],
        *,
        primary: bool = False,
        compact: bool = False,
        danger: bool = False,
    ) -> Any:
        tk = self.tk
        bg = CYAN if primary else (RED if danger else PANEL_2)
        fg = "#001018" if primary else TEXT
        active = "#52e7ff" if primary else ("#ff7a83" if danger else "#24303a")
        button = tk.Button(
            parent,
            text=text,
            command=command,
            bg=bg,
            fg=fg,
            activebackground=active,
            activeforeground=fg,
            bd=0,
            relief="flat",
            cursor="hand2",
            font=("Segoe UI Semibold" if primary else "Segoe UI", 10),
            padx=12 if compact else 18,
            pady=6 if compact else 10,
        )
        button.bind("<Enter>", lambda _e, b=button: b.configure(bg=active))
        button.bind("<Leave>", lambda _e, b=button, c=bg: b.configure(bg=c))
        return button

    def _round_window(self, window: Any) -> None:
        """Best-effort Windows 11 rounded corners for PCC-owned borderless windows."""
        if os.name != "nt":
            return
        try:
            import ctypes
            window.update_idletasks()
            hwnd = int(window.winfo_id())
            parent_hwnd = int(ctypes.windll.user32.GetParent(hwnd))
            target = parent_hwnd or hwnd
            preference = ctypes.c_int(2)  # DWMWCP_ROUND
            ctypes.windll.dwmapi.DwmSetWindowAttribute(target, 33, ctypes.byref(preference), ctypes.sizeof(preference))
        except Exception:
            pass

    def _center_modal(self, dialog: Any, width: int, height: int) -> None:
        dialog.update_idletasks()
        try:
            px = self.window.winfo_rootx()
            py = self.window.winfo_rooty()
            pw = self.window.winfo_width()
            ph = self.window.winfo_height()
            x = px + max(0, (pw - width) // 2)
            y = py + max(0, (ph - height) // 2)
        except Exception:
            sw, sh = dialog.winfo_screenwidth(), dialog.winfo_screenheight()
            x, y = max(0, (sw - width) // 2), max(0, (sh - height) // 2)
        dialog.geometry(f"{width}x{height}+{x}+{y}")


    def _show_owned_modal(self, dialog: Any, *, focus_widget: Any | None = None) -> None:
        """Show a PCC-owned modal above its owner without leaving it permanently topmost."""
        dialog.deiconify()
        dialog.update_idletasks()
        try:
            dialog.transient(self.window)
        except Exception:
            pass
        try:
            dialog.attributes("-topmost", True)
        except Exception:
            pass
        dialog.lift()
        try:
            dialog.grab_set()
        except Exception:
            pass
        try:
            (focus_widget or dialog).focus_force()
        except Exception:
            pass
        # Give Windows one message-loop turn to establish foreground ownership, then
        # remove topmost so the modal behaves like a normal owned application window.
        def normalize() -> None:
            try:
                dialog.attributes("-topmost", False)
                dialog.lift()
            except Exception:
                pass
        dialog.after(125, normalize)

    def _popup(self, title: str, message: str, *, kind: str = "info", confirm: bool = False, parent: Any | None = None) -> bool:
        tk = self.tk
        host = parent or self.window
        dialog = tk.Toplevel(host)
        dialog.withdraw()
        dialog.configure(bg=BG)
        dialog.overrideredirect(True)
        dialog.transient(host)
        dialog.resizable(False, False)

        accent = RED if kind == "error" else (YELLOW if kind == "warning" else (GREEN if kind == "success" else CYAN))
        outer = tk.Frame(dialog, bg=accent, padx=1, pady=1)
        outer.pack(fill="both", expand=True)
        shell = tk.Frame(outer, bg=PANEL)
        shell.pack(fill="both", expand=True)
        tk.Frame(shell, bg=accent, height=4).pack(fill="x")
        tk.Label(shell, text=title, bg=PANEL, fg=TEXT, font=("Segoe UI Semibold", 13), anchor="w").pack(fill="x", padx=18, pady=(16, 6))
        tk.Label(shell, text=message, bg=PANEL, fg=MUTED, font=("Segoe UI", 10), anchor="w", justify="left", wraplength=520).pack(fill="both", expand=True, padx=18, pady=(0, 14))
        result = [False]
        def close(value: bool) -> None:
            result[0] = value
            try:
                dialog.grab_release()
            except Exception:
                pass
            dialog.destroy()
        actions = tk.Frame(shell, bg=PANEL)
        actions.pack(fill="x", padx=16, pady=(0, 16))
        if confirm:
            self._button(actions, "Cancel", lambda: close(False), compact=True).pack(side="right", padx=(8, 0))
            self._button(actions, "Continue", lambda: close(True), primary=True, compact=True).pack(side="right")
        else:
            self._button(actions, "OK", lambda: close(True), primary=True, compact=True).pack(side="right")
        dialog.bind("<Escape>", lambda _e: close(False))
        dialog.bind("<Return>", lambda _e: close(True))
        dialog.protocol("WM_DELETE_WINDOW", lambda: close(False))
        self._center_modal(dialog, 570, 250 if len(message) < 380 else 310)
        self._round_window(dialog)
        self._show_owned_modal(dialog)
        host.wait_window(dialog)
        return bool(result[0])


    def _section_title(self, parent: Any, title: str, subtitle: str = "") -> None:
        tk = self.tk
        tk.Label(parent, text=title, bg=PANEL, fg=TEXT, font=("Segoe UI Semibold", 15)).pack(anchor="w")
        if subtitle:
            tk.Label(parent, text=subtitle, bg=PANEL, fg=MUTED, font=("Segoe UI", 8), wraplength=520, justify="left").pack(anchor="w", pady=(3, 10))

    def _build_header_health_rail(self, parent: Any) -> None:
        """Compact status LEDs shown only while Project Workspace is active."""
        tk = self.tk
        keys = ("Git", "GREEN", "Updates", "Hygiene", "PCC", "Runtime", "Sync")
        rail = tk.Frame(parent, bg=BG)
        rail.pack(fill="both", expand=True)
        for col, key in enumerate(keys):
            cell = tk.Frame(rail, bg=BG)
            cell.grid(row=0, column=col, sticky="nsew", padx=3, pady=(7, 4))
            rail.grid_columnconfigure(col, weight=1, uniform="header-health")
            tk.Label(
                cell,
                text=key,
                bg=BG,
                fg=MUTED,
                font=("Segoe UI Semibold", 7),
            ).pack()
            line = tk.Frame(cell, bg=BG)
            line.pack(pady=(2, 0))
            led = tk.Canvas(line, width=10, height=10, bg=BG, highlightthickness=0, bd=0)
            led.pack(side="left", padx=(0, 4))
            oval = led.create_oval(2, 2, 8, 8, fill=MUTED, outline="")
            value = tk.Label(
                line,
                text="Loading",
                bg=BG,
                fg=CYAN,
                font=("Segoe UI Semibold", 8),
            )
            value.pack(side="left")
            self._status_values[key] = value
            self._status_leds[key] = (led, oval)

    def _set_workspace_sashes(self) -> None:
        panes = getattr(self, "workspace_panes", None)
        if panes is None:
            return
        try:
            panes.update_idletasks()
            width = max(900, panes.winfo_width())
            # left ~13%, middle to ~52%, console gets the remaining ~48%.
            panes.sash_place(0, max(145, int(width * 0.13)), 0)
            panes.sash_place(1, max(500, int(width * 0.52)), 0)
        except Exception:
            pass

    def _scrollable_page_body(self, parent: Any, page_name: str) -> Any:
        """Create a bounded vertically scrollable page body for command-heavy dashboards.

        The shell remains fixed in the workspace while only the page content scrolls.
        This prevents large command catalogs from clipping buttons or resizing the
        application when the middle pane is narrow.
        """
        tk = self.tk
        canvas = tk.Canvas(
            parent,
            bg=PANEL,
            highlightthickness=0,
            bd=0,
            relief="flat",
        )
        scrollbar = self._dark_scrollbar(parent, orient="vertical", command=canvas.yview)
        canvas.configure(yscrollcommand=scrollbar.set, takefocus=True)
        body = tk.Frame(canvas, bg=PANEL)
        window_id = canvas.create_window((0, 0), window=body, anchor="nw")

        def sync_region(_event: Any = None) -> None:
            bbox = canvas.bbox("all")
            if bbox:
                canvas.configure(scrollregion=bbox)

        def sync_width(event: Any) -> None:
            canvas.itemconfigure(window_id, width=max(1, int(event.width)))
            sync_region()

        body.bind("<Configure>", sync_region, add="+")
        canvas.bind("<Configure>", sync_width, add="+")
        canvas.bind("<Button-1>", lambda _event: canvas.focus_set(), add="+")
        body.bind("<Button-1>", lambda _event: canvas.focus_set(), add="+")
        canvas.pack(side="left", fill="both", expand=True)
        scrollbar.pack(side="right", fill="y", padx=(8, 1), pady=1)
        self._page_canvases[page_name] = canvas
        return body

    @staticmethod
    def _widget_is_descendant(widget: Any, ancestor: Any) -> bool:
        current = widget
        while current is not None:
            if current == ancestor:
                return True
            try:
                parent_name = current.winfo_parent()
                if not parent_name:
                    return False
                current = current.nametowidget(parent_name)
            except Exception:
                return False
        return False

    def _scroll_active_page(self, event: Any) -> None:
        canvas = self._page_canvases.get(self._active_page)
        body = self._page_bodies.get(self._active_page)
        if canvas is None or body is None or not canvas.winfo_ismapped():
            return
        if not self._widget_is_descendant(getattr(event, "widget", None), body) and getattr(event, "widget", None) != canvas:
            return
        delta = int(getattr(event, "delta", 0) or 0)
        if delta:
            canvas.yview_scroll(-1 if delta > 0 else 1, "units")

    @staticmethod
    def _scroll_key_should_stay_with_widget(widget: Any) -> bool:
        if widget is None:
            return False
        try:
            widget_class = str(widget.winfo_class()).lower()
        except Exception:
            return False
        return any(
            token in widget_class
            for token in ("entry", "text", "treeview", "listbox", "spinbox", "combobox")
        )

    def _keyboard_scroll_target(self, event: Any) -> Any | None:
        canvas = self._page_canvases.get(self._active_page)
        body = self._page_bodies.get(self._active_page)
        if canvas is None or body is None or not canvas.winfo_ismapped():
            return None
        widget = getattr(event, "widget", None)
        if self._scroll_key_should_stay_with_widget(widget):
            return None
        if widget is not None and not self._widget_is_descendant(widget, body) and widget != canvas:
            # A command button in the active page still counts because it is a descendant
            # of the scroll body; unrelated controls retain their normal keyboard behavior.
            return None
        return canvas

    def _scroll_active_page_key(self, event: Any, amount: int, units: str) -> str | None:
        canvas = self._keyboard_scroll_target(event)
        if canvas is None:
            return None
        canvas.yview_scroll(amount, units)
        return "break"

    def _scroll_active_page_home_end(self, event: Any, fraction: float) -> str | None:
        canvas = self._keyboard_scroll_target(event)
        if canvas is None:
            return None
        canvas.yview_moveto(fraction)
        return "break"

    def _responsive_action_grid(
        self,
        body: Any,
        buttons: Sequence[Any],
        *,
        preferred_columns: int,
        minimum_cell_width: int = 185,
    ) -> None:
        """Reflow action buttons without changing their geometry on hover."""
        state = {"columns": 0}

        def layout(event: Any = None) -> None:
            width = max(1, int(getattr(event, "width", body.winfo_width()) or 1))
            columns = max(1, min(preferred_columns, width // max(1, minimum_cell_width)))
            if columns == state["columns"] and event is not None:
                return
            state["columns"] = columns
            for col in range(max(preferred_columns, 4)):
                body.grid_columnconfigure(col, weight=1 if col < columns else 0, uniform="actions" if col < columns else "")
            wrap = max(130, (width // columns) - 28)
            for index, button in enumerate(buttons):
                button.grid_forget()
                row, col = divmod(index, columns)
                button.configure(wraplength=wrap, justify="center")
                button.grid(row=row, column=col, sticky="ew", padx=4, pady=4)

        body.bind("<Configure>", layout, add="+")
        body.after_idle(layout)

    def _toolbar_grid(
        self,
        parent: Any,
        actions: Sequence[tuple[str, Callable[[], None], bool, bool]],
        *,
        preferred_columns: int = 4,
        minimum_cell_width: int = 150,
    ) -> Any:
        """Responsive replacement for fixed horizontal button strips."""
        try:
            bg = parent.cget("bg")
        except Exception:
            bg = PANEL
        body = self.tk.Frame(parent, bg=bg)
        body.pack(fill="x")
        buttons = [
            self._button(
                body,
                label,
                command,
                primary=primary,
                compact=True,
                danger=danger,
            )
            for label, command, primary, danger in actions
        ]
        self._responsive_action_grid(
            body,
            buttons,
            preferred_columns=max(1, preferred_columns),
            minimum_cell_width=minimum_cell_width,
        )
        return body

    def _action_grid(self, parent: Any, actions: Sequence[tuple[str, Callable[[], None], bool]], *, columns: int = 2) -> Any:
        tk = self.tk
        body = tk.Frame(parent, bg=PANEL)
        body.pack(fill="x", padx=4, pady=4)
        buttons = [
            self._button(body, label, command, primary=primary, compact=True)
            for label, command, primary in actions
        ]
        self._responsive_action_grid(body, buttons, preferred_columns=max(1, columns))
        return body

    def _register_category_command_surface(
        self,
        parent: Any,
        categories: Sequence[str],
        *,
        title: str = "Registered Commands",
    ) -> Any:
        panel = self._panel(parent, title)
        panel.pack(fill="x", pady=(0, 8))
        host = self.tk.Frame(panel, bg=PANEL)
        host.pack(fill="x", padx=4, pady=4)
        record = (host, tuple(categories))
        self._category_command_hosts.append(record)
        self._render_category_command_host(host, tuple(categories))
        return panel

    def _render_category_command_host(self, host: Any, categories: tuple[str, ...]) -> None:
        for child in host.winfo_children():
            child.destroy()
        rows = [row for row in self._command_rows if row.category in categories]
        if not rows:
            self.tk.Label(
                host,
                text="No commands in this category for the selected project.",
                bg=PANEL,
                fg=MUTED,
                font=("Segoe UI", 8),
            ).pack(anchor="w", padx=5, pady=5)
            return
        actions: list[tuple[str, Callable[[], None], bool]] = []
        for row in rows:
            actions.append((
                row.label,
                lambda current=row: self._start_registered_command(current),
                row.risk == "read_only",
            ))
        body = self.tk.Frame(host, bg=PANEL)
        body.pack(fill="x")
        buttons = [
            self._button(body, label, command, primary=primary, compact=True)
            for label, command, primary in actions
        ]
        self._responsive_action_grid(body, buttons, preferred_columns=3, minimum_cell_width=175)

    def _reload_category_command_surfaces(self) -> None:
        for host, categories in self._category_command_hosts:
            self._render_category_command_host(host, categories)


    def _build_health_rail(self, parent: Any) -> None:
        # Legacy compatibility shim. Health is now rendered in the application header.
        return

    def _build_dashboard(self, parent: Any) -> None:
        tk = self.tk
        self._section_title(
            parent,
            "Dashboard",
            "Selected-project authority and the most useful development state at a glance.",
        )
        summary = self._panel(parent, "Current Authority")
        summary.pack(fill="both", expand=True)
        self.summary_text = tk.Text(
            summary,
            bg=PANEL,
            fg=TEXT,
            insertbackground=TEXT,
            selectbackground="#21404a",
            selectforeground=TEXT,
            bd=0,
            relief="flat",
            font=("Consolas", 9),
            height=16,
            wrap="word",
        )
        self.summary_text.pack(fill="both", expand=True, padx=12, pady=(4, 12))
        self.summary_text.configure(state="disabled")


    def _build_build_page(self, parent: Any) -> None:
        self._section_title(parent, "Build & Run", "Every registered gate, build, test and runtime operation for the selected project.")
        self._register_category_command_surface(parent, ("gate", "build", "test", "run"), title="Build / Test / Run Commands")
        panel = self._panel(parent, "Project Shortcuts")
        panel.pack(fill="x")
        self._action_grid(panel, (
            ("Open Project Folder", lambda: open_path(self.root_path), True),
            ("Refresh Health", self._refresh_status_async, False),
        ))


    def _build_updates_page(self, parent: Any) -> None:
        self._section_title(parent, "Updates", "Validated patch intake, evidence and recovery.")
        self._register_category_command_surface(parent, ("updates",), title="Update Commands")
        panel = self._panel(parent, "Update Shortcuts")
        panel.pack(fill="x")
        self._action_grid(panel, (
            ("Refresh Health", self._refresh_status_async, True),
            ("Open Patch Artifacts", lambda: open_path(self.root_path / "artifacts" / "patches"), False),
        ))
        history = self._panel(parent, "History")
        history.pack(fill="x", pady=(10, 0))
        patch_root = lambda: self.root_path / "artifacts" / "patches"
        self._action_grid(history, (
            ("Applied", lambda: open_path(patch_root() / "applied"), False),
            ("Failed", lambda: open_path(patch_root() / "failed"), False),
            ("Receipts", lambda: open_path(patch_root() / "receipts"), False),
            ("Backups", lambda: open_path(patch_root() / "backups"), False),
        ))


    def _build_source_page(self, parent: Any) -> None:
        self._section_title(parent, "Source Control", "Every registered repository/Git command, including GREEN-gated mutation paths.")

        identity = self._panel(parent, "Git Author Identity")
        identity.pack(fill="x", pady=(0, 10))
        self.git_identity_label = self.tk.Label(
            identity,
            text="Checking Git identity...",
            bg=PANEL,
            fg=MUTED,
            justify="left",
            anchor="w",
            font=("Segoe UI", 9),
        )
        self.git_identity_label.pack(fill="x", padx=12, pady=(4, 8))
        self._action_grid(identity, (
            ("Configure Git Identity", self._configure_git_identity, True),
            ("Trust Current Checkout", self._trust_git_checkout, False),
            ("Refresh Identity", self._refresh_git_identity_panel, False),
        ))
        self._refresh_git_identity_panel()

        self._register_category_command_surface(parent, ("source-control",), title="Source-Control Commands")


    def _git_config_value(self, scope: str, key: str) -> str:
        try:
            cp = subprocess.run(
                ["git", "-C", str(self.root_path), "config", f"--{scope}", "--get", key],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=5,
                check=False,
            )
            return cp.stdout.strip() if cp.returncode == 0 else ""
        except Exception:
            return ""

    def _project_git_identity_authority(self) -> dict[str, str]:
        """Return the repository-declared author identity, if one is valid.

        Cortex repositories may carry a portable GitHub authority contract under
        config/cortex/github_authority.v1.json.  The backend applies that author
        as repository-local Git configuration during governed commit operations.
        The GUI must recognize the same authority *before* the commit begins;
        otherwise it repeatedly asks for an identity that the project already
        owns.
        """
        path = self.root_path / "config" / "cortex" / "github_authority.v1.json"
        try:
            data = json.loads(path.read_text(encoding="utf-8-sig"))
        except Exception:
            return {}
        if not isinstance(data, dict) or data.get("schema") != "cortex.github_authority.v1":
            return {}
        author = data.get("author")
        if not isinstance(author, dict):
            return {}
        name = str(author.get("name") or "").strip()
        email = str(author.get("email") or "").strip()
        scope = str(author.get("scope") or "local").strip().casefold()
        if not name or not email or "@" not in email:
            return {}
        if scope not in {"local", "global"}:
            scope = "local"
        return {"name": name, "email": email, "scope": scope}

    def _git_identity_snapshot(self) -> dict[str, str | bool]:
        local_name = self._git_config_value("local", "user.name")
        local_email = self._git_config_value("local", "user.email")
        global_name = self._git_config_value("global", "user.name")
        global_email = self._git_config_value("global", "user.email")
        authority = self._project_git_identity_authority()

        local_complete = bool(local_name and local_email)
        authority_complete = bool(authority.get("name") and authority.get("email"))
        global_complete = bool(global_name and global_email)

        # Repository-local identity is authoritative once present.  For a
        # governed portable project, the repository-declared authority comes
        # next because the backend will materialize it locally at commit time.
        # A machine-global Git identity remains a fallback for ordinary repos.
        if local_complete:
            effective_name = local_name
            effective_email = local_email
            source = "local"
        elif authority_complete:
            effective_name = str(authority["name"])
            effective_email = str(authority["email"])
            source = "project-authority"
        elif global_complete:
            effective_name = global_name
            effective_email = global_email
            source = "global"
        else:
            effective_name = ""
            effective_email = ""
            source = "missing"

        return {
            "configured": bool(effective_name and effective_email),
            "name": effective_name,
            "email": effective_email,
            "source": source,
            "local_name": local_name,
            "local_email": local_email,
            "global_name": global_name,
            "global_email": global_email,
            "authority_name": authority.get("name", ""),
            "authority_email": authority.get("email", ""),
            "authority_scope": authority.get("scope", ""),
            "authority_pending_local_apply": bool(authority_complete and not local_complete),
        }

    def _refresh_git_identity_panel(self) -> None:
        if not hasattr(self, "git_identity_label"):
            return
        snap = self._git_identity_snapshot()
        if snap["configured"]:
            if snap.get("source") == "project-authority" and snap.get("authority_pending_local_apply"):
                text = f"Project authority: {snap['name']} <{snap['email']}> — applies locally on first governed commit"
            else:
                text = f"Configured ({snap['source']}): {snap['name']} <{snap['email']}>"
            self.git_identity_label.configure(
                text=text,
                fg=GREEN,
            )
        else:
            self.git_identity_label.configure(
                text="Not configured. Certified commits are blocked until an author name and email are set.",
                fg=YELLOW,
            )

    def _trust_git_checkout(self) -> None:
        if self._busy:
            self._popup("Git Checkout Trust", "Wait for the active PCC job to finish before changing Git checkout trust.", kind="warning")
            return
        if not self._popup(
            "Trust Current Checkout",
            "Trust this Cortex checkout for the current Windows user on this machine?\n\n"
            "This adds only the current repository path to Git safe.directory. It does not disable Git ownership checks globally and does not modify project source.",
            kind="warning",
            confirm=True,
        ):
            return
        self._start_command("git-trust", label="git-trust")

    def _configure_git_identity(self) -> None:
        if self._busy:
            self._popup("Git Identity", "Wait for the active PCC job to finish before changing Git identity.", kind="warning")
            return
        tk = self.tk
        ttk = self.ttk
        snap = self._git_identity_snapshot()
        dialog = tk.Toplevel(self.window)
        dialog.withdraw()
        dialog.title("Configure Git Identity")
        dialog.configure(bg=BG)
        dialog.transient(self.window)

        shell = tk.Frame(dialog, bg=PANEL, highlightthickness=1, highlightbackground=BORDER)
        shell.pack(fill="both", expand=True, padx=1, pady=1)
        tk.Label(shell, text="Git Author Identity", bg=PANEL, fg=TEXT, font=("Segoe UI Semibold", 13)).pack(anchor="w", padx=18, pady=(16, 4))
        tk.Label(
            shell,
            text="Git requires an author identity before Cortex can commit a certified GREEN checkpoint. Global applies to all repositories for this Windows user; Local applies only to the selected project.",
            bg=PANEL, fg=MUTED, font=("Segoe UI", 9), justify="left", wraplength=650,
        ).pack(anchor="w", padx=18, pady=(0, 12))

        form = tk.Frame(shell, bg=PANEL)
        form.pack(fill="x", padx=18)
        name_var = tk.StringVar(value=str(snap.get("name") or ""))
        email_var = tk.StringVar(value=str(snap.get("email") or ""))
        # Repository-local is the portable/default choice. It travels in the
        # checkout's .git/config when the external drive changes computers or
        # drive letters. Global remains available as an explicit override.
        default_scope = "local"
        scope_var = tk.StringVar(value=default_scope)

        for label, variable in (("Name", name_var), ("Email", email_var)):
            tk.Label(form, text=label, bg=PANEL, fg=MUTED, font=("Segoe UI Semibold", 9)).pack(anchor="w", pady=(0, 4))
            entry = tk.Entry(
                form, textvariable=variable, bg=PANEL_2, fg=TEXT, insertbackground=TEXT,
                selectbackground="#21404a", selectforeground=TEXT, bd=0, relief="flat", font=("Segoe UI", 10),
            )
            entry.pack(fill="x", ipady=7, pady=(0, 10))

        tk.Label(form, text="Scope", bg=PANEL, fg=MUTED, font=("Segoe UI Semibold", 9)).pack(anchor="w", pady=(0, 4))
        scope = ttk.Combobox(form, textvariable=scope_var, values=["global", "local"], state="readonly", width=16)
        scope.pack(anchor="w", pady=(0, 8))

        def close() -> None:
            try:
                dialog.grab_release()
            except Exception:
                pass
            dialog.destroy()

        def save() -> None:
            name = name_var.get().strip()
            email = email_var.get().strip()
            selected_scope = scope_var.get().strip().casefold() or "local"
            if not name:
                self._popup("Git Identity", "Enter the author name used for Git commits.", kind="warning", parent=dialog)
                return
            if not email or "@" not in email or any(ch.isspace() for ch in email):
                self._popup("Git Identity", "Enter a valid Git author email address.", kind="warning", parent=dialog)
                return
            close()
            self._start_command(
                "git-identity",
                ["--git-name", name, "--git-email", email, "--git-scope", selected_scope],
                label="git-identity",
            )

        actions = tk.Frame(shell, bg=PANEL)
        actions.pack(fill="x", padx=18, pady=(10, 16))
        self._button(actions, "Cancel", close, compact=True).pack(side="right", padx=(8, 0))
        self._button(actions, "Save Git Identity", save, primary=True, compact=True).pack(side="right")
        dialog.bind("<Escape>", lambda _e: close())
        dialog.protocol("WM_DELETE_WINDOW", close)
        self._center_modal(dialog, 700, 390)
        self._round_window(dialog)
        dialog.deiconify()
        dialog.lift()
        dialog.grab_set()

    def _build_diagnostics_page(self, parent: Any) -> None:
        self._section_title(parent, "Diagnostics", "Health, recovery, validation and evidence-generation operations.")
        self._register_category_command_surface(parent, ("diagnostics",), title="Diagnostic Commands")
        panel = self._panel(parent, "Evidence Shortcuts")
        panel.pack(fill="x")
        self._action_grid(panel, (
            ("Open Debug Folder", lambda: open_path(self.root_path / "artifacts" / "debug"), True),
            ("Open Artifacts", lambda: open_path(self.root_path / "artifacts"), False),
        ))

    def _build_storage_commands_page(self, parent: Any) -> None:
        self._section_title(
            parent,
            "Storage & Vault",
            "Shared dependency paths, project mirrors, verification, retention and CAS lifecycle commands.",
        )
        self._register_category_command_surface(parent, ("storage",), title="Storage / Vault Commands")
        panel = self._panel(parent, "Vault Shortcuts")
        panel.pack(fill="x")
        self._action_grid(panel, (
            ("Open Project Vault", lambda: open_path(vault_project_dir(self.root_path)), True),
            ("Open Global Vault", lambda: open_path(global_vault_root(self.root_path)), False),
        ))

    def _build_tooling_page(self, parent: Any) -> None:
        self._section_title(
            parent,
            "Tooling",
            "Formatting, metadata, Forge candidate, migration and cross-project audit commands.",
        )
        self._register_category_command_surface(parent, ("tooling", "project", "advanced"), title="Tooling / Project Commands")

    def _build_logs_page(self, parent: Any) -> None:
        tk = self.tk
        self._section_title(parent, "Logs", "Expanded live view. The same stream remains visible in the Project Console at all times.")
        toolbar = tk.Frame(parent, bg=BG)
        toolbar.pack(fill="x", pady=(0, 8))
        self._button(toolbar, "Copy All", lambda: self._copy_all(self.log_text), primary=True, compact=True).pack(side="left", padx=(0, 6))
        self._button(toolbar, "Copy Selection", lambda: self._copy_selection(self.log_text), compact=True).pack(side="left", padx=6)
        self._button(toolbar, "Clear", self._clear_log, compact=True).pack(side="left", padx=6)
        self._button(toolbar, "Open Active Log", self._open_active_log, compact=True).pack(side="left", padx=6)
        self._button(toolbar, "Open Session Logs", lambda: open_path(self.root_path / "artifacts" / "logs" / "sessions"), compact=True).pack(side="left", padx=6)
        self._button(toolbar, "Open Latest Debug", self._open_latest_debug, compact=True).pack(side="left", padx=6)

        frame = self._panel(parent)
        frame.pack(fill="both", expand=True)
        self.log_text = tk.Text(frame, bg="#07090b", fg=TEXT, insertbackground=TEXT, bd=0, relief="flat", font=("Consolas", 9), wrap="word")
        scroll = self._dark_scrollbar(frame, orient="vertical", command=self.log_text.yview)
        self.log_text.configure(yscrollcommand=scroll.set)
        self.log_text.pack(side="left", fill="both", expand=True, padx=(10, 0), pady=10)
        scroll.pack(side="right", fill="y", pady=10, padx=(0, 8))
        self._configure_log_tags(self.log_text)


    def _build_commands_page(self, parent: Any) -> None:
        tk = self.tk
        ttk = self.ttk
        self._section_title(
            parent,
            "Command Registry",
            "One command authority for project-contract and project-provider operations. Search, filter and run the same commands exposed through the console composer.",
        )
        controls = tk.Frame(parent, bg=BG)
        controls.pack(fill="x", pady=(0, 8))
        self.commands_filter_var = tk.StringVar()
        self.commands_category_var = tk.StringVar(value="All")
        search = tk.Entry(
            controls,
            textvariable=self.commands_filter_var,
            bg=PANEL_2,
            fg=TEXT,
            insertbackground=TEXT,
            selectbackground="#21404a",
            selectforeground=TEXT,
            bd=0,
            relief="flat",
            font=("Segoe UI", 9),
        )
        search.pack(side="left", fill="x", expand=True, ipady=6, padx=(0, 8))
        search.bind("<KeyRelease>", lambda _event: self._reload_registered_commands())
        categories = ["All", *sorted({row.category for row in self._command_rows if row.category})]
        self.commands_category_box = ttk.Combobox(
            controls,
            textvariable=self.commands_category_var,
            values=categories,
            state="readonly",
            width=18,
        )
        self.commands_category_box.pack(side="left", padx=(0, 8))
        self.commands_category_box.bind("<<ComboboxSelected>>", lambda _event: self._reload_registered_commands())
        self._button(controls, "Run Selected", self._run_selected_registered_command, primary=True, compact=True).pack(side="left", padx=(0, 6))
        self._button(controls, "Refresh", self._refresh_command_catalog, compact=True).pack(side="left")

        panel = self._panel(parent)
        panel.pack(fill="both", expand=True)
        self.commands_tree = ttk.Treeview(
            panel,
            columns=("category", "key", "label", "risk", "source", "program"),
            show="headings",
            selectmode="browse",
        )
        for key, title, width in (
            ("category", "Category", 110),
            ("key", "Command", 175),
            ("label", "Label", 235),
            ("risk", "Risk", 95),
            ("source", "Source", 115),
            ("program", "Program", 90),
        ):
            self.commands_tree.heading(key, text=title)
            self.commands_tree.column(key, width=width, anchor="w")
        yscroll = self._dark_scrollbar(panel, orient="vertical", command=self.commands_tree.yview)
        self.commands_tree.configure(yscrollcommand=yscroll.set)
        self.commands_tree.pack(side="left", fill="both", expand=True, padx=(8, 0), pady=8)
        yscroll.pack(side="right", fill="y", pady=8, padx=(0, 8))
        self.commands_tree.bind("<Double-1>", lambda _event: self._run_selected_registered_command())
        self._reload_registered_commands()

    # ------------------------------------------------------------------
    # App / project navigation
    # ------------------------------------------------------------------

    def _show_app_tab(self, name: str) -> None:
        for key, frame in self._app_frames.items():
            frame.pack_forget()
            btn = self._app_tab_buttons.get(key)
            if btn:
                btn.configure(bg=PANEL, fg=TEXT)
        self._app_frames[name].pack(fill="both", expand=True)
        self._app_tab_buttons[name].configure(bg=PANEL_2, fg=CYAN)
        if name == "Chat":
            self._refresh_chat_header()
            self._refresh_chat_history()
            if hasattr(self, "chat_input"):
                self.chat_input.focus_set()
        elif name == "Vault / Forge" and not self._vault_tree_initialized:
            self._vault_tree_initialized = True
            # Defer the first filesystem view until after the tab is painted.
            self.window.after(1, self._vault_refresh_tree)

        if hasattr(self, "header_health_rail"):
            if name == "Project Workspace":
                if not self.header_health_rail.winfo_ismapped():
                    self.header_health_rail.pack(fill="both", expand=True)
            else:
                self.header_health_rail.pack_forget()

    def _show_page(self, page: str) -> None:
        for name, frame in self._page_frames.items():
            frame.pack_forget()
            btn = self._nav_buttons.get(name)
            if btn:
                btn.configure(bg=PANEL, fg=TEXT)
        self._active_page = page
        self._page_frames[page].pack(fill="both", expand=True)
        canvas = self._page_canvases.get(page)
        if canvas is not None:
            canvas.yview_moveto(0.0)
        if page in self._nav_buttons:
            self._nav_buttons[page].configure(bg=PANEL_2, fg=CYAN)

    def _bind_project_backend(self) -> None:
        try:
            self.backend = BackendClient(self.root_path, self.contract)
            self.backend_error = ""
        except SurfaceError as exc:
            self.backend = None
            self.backend_error = str(exc)

    def _activate_project(self, root: Path) -> None:
        if self._busy:
            self._popup("Project Control Center", "Finish or stop the active PCC job before switching projects.", kind="warning")
            return
        target_root = root.resolve()
        activation_hygiene: dict[str, Any] = {}
        try:
            activation_hygiene = repo_hygiene_prepare(target_root, apply=False)
        except Exception as exc:
            activation_hygiene = {"moved": 0, "error": str(exc)}
        try:
            contract = ProjectContract.load(target_root)
        except Exception as exc:
            self._popup("Unable to Load Project", f"{root}\n\n{exc}", kind="error")
            return
        self.root_path = root.resolve()
        self.contract = contract
        self._bind_project_backend()
        self.registry.touch(self.root_path)
        self._last_status = {}
        self._update_header()
        self._refresh_command_catalog()
        self._refresh_chat_header()
        self._refresh_chat_history()
        self._render_current_chat()
        self._reset_status_cards()
        self._clear_log()
        if hasattr(self, "vault_tree"):
            self._vault_refresh_tree()
            self._vault_render_summary(vault_latest_summary(self.root_path))
        self._append_log(f"Active project changed to {self.contract.name}.\n", "info")
        self._append_log(f"Root: {self.root_path}\n", "muted")
        if activation_hygiene.get("error"):
            self._append_log(f"[WARN] Project activation hygiene: {activation_hygiene['error']}\n", "warn")
        else:
            moved = int(activation_hygiene.get("moved", 0) or 0)
            self._append_log(f"[PASS] Project activation hygiene preview: {moved} loose operational artifact(s) would move during an explicit operation.\n", "pass")
        if self.backend_error:
            self._append_log(f"PCC adapter: {self.backend_error}\n", "warn")
            self._render_adapter_unavailable()
        else:
            self._refresh_status_async()
        self._show_page("Dashboard")
        self._show_app_tab("Project Workspace")
        self._refresh_projects()

    def _update_header(self) -> None:
        if not hasattr(self, "active_project_label"):
            return
        if self.backend is None:
            adapter = "PCC scan could not bind operations"
        elif self.backend.provider_mode == "auto-contract":
            adapter = "PCC auto-adapter ready"
        else:
            adapter = "PCC native provider ready"
        self.active_project_label.configure(
            text=f"Active: {self.contract.name}  •  {self.contract.kind}  •  {compact_path(self.root_path, 88)}  •  {adapter}"
        )
        self.window.title(f"Project Control Center — {self.contract.name}")

    # ------------------------------------------------------------------
    # Registry
    # ------------------------------------------------------------------
    def _refresh_projects(self) -> None:
        """Refresh the registry table without probing every project provider.

        Project discovery/provider/Vault probing is intentionally deferred until a
        row settles as the current selection.  This keeps cold external drives,
        antivirus scans, stale roots, or a slow project manifest from starving the
        Tk event loop and making row clicks appear to be ignored.
        """
        if not hasattr(self, "projects_tree"):
            return
        self._project_entries_by_id.clear()
        for iid in self.projects_tree.get_children():
            self.projects_tree.delete(iid)
        try:
            entries = self.registry.entries()
        except Exception as exc:
            self._popup("Project Registry", str(exc), kind="error")
            return
        for entry in entries:
            self._project_entries_by_id[entry.registry_id] = entry
            try:
                exists = entry.root.is_dir()
            except OSError:
                exists = False
            tag = "ready" if exists else "missing"
            adapter_text = "Registered" if exists else "Missing root"
            # Keep the list refresh cheap. Detailed catalog/provider state is loaded
            # asynchronously only for the selected project.
            catalog_text = "Select to inspect" if exists else "Unavailable"
            last = entry.last_opened_utc.replace("T", " ")[:19] if entry.last_opened_utc else "—"
            self.projects_tree.insert(
                "", "end", iid=entry.registry_id,
                values=(entry.name, entry.kind, str(entry.root), adapter_text, catalog_text, last),
                tags=(tag,),
            )
        current_id = ProjectRegistry._registry_id(self.root_path)
        if current_id in self._project_entries_by_id:
            self.projects_tree.selection_set(current_id)
            self.projects_tree.focus(current_id)
            self._project_selection_changed()

    def _selected_project(self) -> RegisteredProject | None:
        sel = self.projects_tree.selection()
        if not sel:
            return None
        return self._project_entries_by_id.get(sel[0])

    def _project_selection_changed(self, _event: Any = None) -> None:
        entry = self._selected_project()
        if entry is None:
            self.project_detail.configure(text="Select a registered project.", fg=MUTED)
            return

        # Selection feedback is immediate. Provider discovery and Vault inspection
        # happen after a short debounce on a worker thread so clicks never wait for
        # disk/project probes.
        self._project_probe_generation += 1
        generation = self._project_probe_generation
        if self._project_probe_after is not None:
            try:
                self.window.after_cancel(self._project_probe_after)
            except Exception:
                pass
            self._project_probe_after = None

        self.project_detail.configure(
            text=(
                f"Project    : {entry.name}\n"
                f"Type       : {entry.kind}\n"
                f"Root       : {entry.root}\n"
                "PCC        : Inspecting…\n"
                "Vault      : Inspecting…"
            ),
            fg=TEXT if entry.root.exists() else RED,
        )

        def launch_probe() -> None:
            self._project_probe_after = None

            def work() -> None:
                adapter = "Ready"
                detail_color = TEXT
                discovery: dict[str, Any] = {}
                try:
                    contract = ProjectContract.load(entry.root)
                    backend = BackendClient(entry.root, contract)
                    discovery = contract.raw.get("_pccDiscovery") or {}
                    source = str(discovery.get("source") or "unknown")
                    if source in {"native-project-pcc", "project-powershell-pcc"}:
                        adapter = "Native Project PCC"
                    elif source == "project.control.json":
                        adapter = "Project Contract"
                    else:
                        adapter = "Generated fallback" if backend.provider_mode == "auto-contract" else "Native provider"
                    provider = backend.provider_label
                    catalog = vault_latest_summary(entry.root)
                    catalog_line = f"{catalog.get('files', 0)} files / {catalog.get('duplicateGroups', 0)} duplicate groups" if catalog else "Not cataloged yet"
                    mirror_state = vault_mirror_status(entry.root)
                except Exception as exc:
                    adapter = "Scan incomplete"
                    provider = str(exc)
                    source = "filesystem-scan"
                    try:
                        catalog = vault_latest_summary(entry.root) if entry.root.exists() else None
                        catalog_line = f"{catalog.get('files', 0)} files" if catalog else "Not cataloged yet"
                        mirror_state = vault_mirror_status(entry.root) if entry.root.exists() else {"hasSnapshot": False}
                    except Exception:
                        catalog_line = "Unavailable"
                        mirror_state = {"hasSnapshot": False}
                    detail_color = YELLOW if entry.root.exists() else RED
                passport = self.registry.path.parent / "passports" / f"{ProjectRegistry._registry_id(entry.root)}.json"
                detail = (
                    f"Project    : {entry.name}\n"
                    f"Type       : {entry.kind}\n"
                    f"Root       : {entry.root}\n"
                    f"PCC        : {adapter}\n"
                    f"Discovery  : {source}\n"
                    f"Manifest   : {discovery.get('manifest') or '—'}\n"
                    f"Entrypoint : {discovery.get('entrypoint') or discovery.get('provider') or '—'}\n"
                    f"Fallback   : {discovery.get('fallback') or entry.kind}\n"
                    f"Vault      : {catalog_line}\n"
                    f"Mirror     : {'Current snapshot present' if mirror_state.get('hasSnapshot') else 'Not mirrored yet'}\n"
                    f"Passport   : {passport}\n"
                    f"Provider   : {provider}"
                )
                self._event_q.put(("project-detail", (generation, entry.registry_id, detail, detail_color, adapter, catalog_line)))

            threading.Thread(target=work, daemon=True).start()

        self._project_probe_after = self.window.after(120, launch_probe)

    def _register_project(self) -> None:
        raw = self.filedialog.askdirectory(title="Register Project Root")
        if not raw:
            return
        try:
            entry = self.registry.register(Path(raw), make_active=False)
        except Exception as exc:
            self._popup(
                "Register Project",
                f"The universal PCC could not scan/register this folder.\n\n{exc}",
                kind="error",
            )
            return
        self._refresh_projects()
        if entry.registry_id in self._project_entries_by_id:
            self.projects_tree.selection_set(entry.registry_id)
            self.projects_tree.focus(entry.registry_id)
            self._project_selection_changed()
        self._start_onboarding_scan(entry.root)

    def _start_onboarding_scan(self, root: Path) -> None:
        root = root.resolve()
        self._append_log(f"[INFO] Onboarding scan queued: {root}\n", "info")
        def work() -> None:
            try:
                summary = vault_scan_project(root, deep_hash=False)
                mirror = vault_mirror_project(root, label="project-registration")
                self._event_q.put(("onboard-done", (root, summary, mirror)))
            except Exception as exc:
                self._event_q.put(("onboard-error", (root, str(exc))))
        threading.Thread(target=work, daemon=True).start()

    def _remove_selected_project(self) -> None:
        entry = self._selected_project()
        if entry is None:
            return
        if entry.root.resolve() == self.root_path.resolve():
            if not self._popup("Remove Registration", f"Remove the active project '{entry.name}' from the registry? This does not delete any project files.", kind="warning", confirm=True):
                return
        elif not self._popup("Remove Registration", f"Remove '{entry.name}' from the PCC registry? This does not delete any project files.", kind="warning", confirm=True):
            return
        self.registry.remove(entry.registry_id)
        self._refresh_projects()

    def _open_selected_project(self) -> None:
        entry = self._selected_project()
        if entry is None:
            self._popup("Projects", "Select a project first.")
            return
        self._activate_project(entry.root)

    def _open_selected_project_folder(self) -> None:
        entry = self._selected_project()
        if entry:
            open_path(entry.root)

    # ------------------------------------------------------------------
    # Live output / clipboard
    # ------------------------------------------------------------------
    _SEMANTIC_LOG_RE = re.compile(
        r"\b(PASS(?:ED)?|FAIL(?:ED|URE)?|WARN(?:ING)?|ERROR)\b",
        re.IGNORECASE,
    )
    _ANSI_RE = re.compile(r"\x1B(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1B\\))")

    def _configure_log_tags(self, widget: Any) -> None:
        # Console output stays neutral. Only the semantic result token is colored;
        # punctuation/brackets, paths, commands, hashes and surrounding prose remain
        # the normal console foreground.
        widget.tag_configure("semantic-pass", foreground=GREEN)
        widget.tag_configure("semantic-warn", foreground=YELLOW)
        widget.tag_configure("semantic-fail", foreground=RED)

    @staticmethod
    def _semantic_log_tag(token: str) -> str:
        upper = token.upper()
        if upper.startswith("PASS"):
            return "semantic-pass"
        if upper.startswith("WARN"):
            return "semantic-warn"
        return "semantic-fail"

    def _insert_semantic_log(self, widget: Any, text: str) -> None:
        cursor = 0
        for match in self._SEMANTIC_LOG_RE.finditer(text):
            start, end = match.span()
            if start > cursor:
                widget.insert("end", text[cursor:start])
            token = text[start:end]
            widget.insert("end", token, self._semantic_log_tag(token))
            cursor = end
        if cursor < len(text):
            widget.insert("end", text[cursor:])

    def _append_log(self, text: str, tag: str = "") -> None:
        # `tag` is retained for call-site compatibility, but intentionally does not
        # color the entire line. Semantic token coloring is authoritative.
        for widget_name in ("console_text", "log_text"):
            widget = getattr(self, widget_name, None)
            if widget is None:
                continue
            widget.configure(state="normal")
            self._insert_semantic_log(widget, text)
            widget.see("end")
            widget.configure(state="disabled")

    def _compact_console_stream_line(self, text: str) -> str | None:
        line = self._ANSI_RE.sub("", text)
        if hasattr(self, "console_verbose_var") and self.console_verbose_var.get():
            self._console_compacting_diff = False
            return line
        stripped = line.strip()
        if self._console_compacting_diff:
            if stripped.startswith(("[PASS]", "[FAIL]", "[WARN]", "[INFO]", "===", "GREEN SOURCE MARKER", "error:", "warning:")):
                self._console_compacting_diff = False
            else:
                return None
        if stripped.startswith("Diff in "):
            self._console_compacting_diff = True
            return "[INFO] rustfmt diff compacted in Project Console; full output remains in the expanded log and durable command-output artifact.\n"
        if stripped.startswith("PCC_RESULT_JSON="):
            return None
        if stripped.startswith("test_") and stripped.endswith("... ok"):
            return None
        if "TEST-0" in stripped and any(token in stripped for token in ("PATCH DETECTED", "APPLIED:", "Patch TEST-")):
            return None
        return line

    def _append_stream_log(self, text: str, tag: str = "") -> None:
        clean = self._ANSI_RE.sub("", text)
        expanded = getattr(self, "log_text", None)
        if expanded is not None:
            expanded.configure(state="normal")
            self._insert_semantic_log(expanded, clean)
            expanded.see("end")
            expanded.configure(state="disabled")
        compact = self._compact_console_stream_line(clean)
        if compact is None:
            return
        console = getattr(self, "console_text", None)
        if console is not None:
            console.configure(state="normal")
            self._insert_semantic_log(console, compact)
            console.see("end")
            console.configure(state="disabled")

    def _clear_log(self) -> None:
        for widget_name in ("console_text", "log_text"):
            widget = getattr(self, widget_name, None)
            if widget is None:
                continue
            widget.configure(state="normal")
            widget.delete("1.0", "end")
            widget.configure(state="disabled")

    def _copy_to_clipboard(self, text: str, label: str) -> None:
        self.window.clipboard_clear()
        self.window.clipboard_append(text)
        self.window.update_idletasks()
        if hasattr(self, "footer"):
            self.footer.configure(text=f"[Copied:{label}] [{len(text)} chars]", fg=GREEN)

    def _copy_all(self, widget: Any) -> None:
        text = widget.get("1.0", "end-1c")
        if not text:
            self._popup("Copy Console", "There is no console output to copy.")
            return
        self._copy_to_clipboard(text, "All Console Output")

    def _copy_selection(self, widget: Any) -> None:
        try:
            text = widget.get("sel.first", "sel.last")
        except self.tk.TclError:
            self._popup("Copy Selection", "Select console text first, or use Copy All.")
            return
        self._copy_to_clipboard(text, "Console Selection")

    def _open_active_log(self) -> None:
        raw = str(((self._last_status.get("session") or {}).get("log") or "")).strip()
        if raw:
            path = Path(raw)
            if path.exists():
                reveal_file(path)
                return
        open_path(self.root_path / "artifacts" / "logs" / "sessions")

    # ------------------------------------------------------------------
    # Status
    # ------------------------------------------------------------------
    def _refresh_clicked(self) -> None:
        self._refresh_projects()
        self._refresh_status_async()

    def _refresh_status_async(self) -> None:
        if self._busy:
            return
        if self.backend is None:
            self._render_adapter_unavailable()
            return
        self._status_generation += 1
        generation = self._status_generation
        self.refresh_btn.configure(state="disabled")
        self.footer.configure(text="[Status:Refreshing]", fg=CYAN)

        def work() -> None:
            try:
                status = self.backend.status() if self.backend is not None else {}
                self._event_q.put(("status", (generation, status)))
            except Exception as exc:
                self._event_q.put(("status-error", (generation, str(exc))))

        threading.Thread(target=work, daemon=True).start()

    def _set_status_card(self, key: str, text: str, color: str) -> None:
        label = self._status_values.get(key)
        if label:
            label.configure(text=text, fg=color)
        led = self._status_leds.get(key)
        if led:
            canvas, oval = led
            # LEDs carry the same authority color as the text. Unknown/loading stays gray/cyan.
            try:
                canvas.itemconfigure(oval, fill=color)
            except Exception:
                pass

    def _reset_status_cards(self) -> None:
        for key in self._status_values:
            self._set_status_card(key, "Loading", CYAN)

    def _render_adapter_unavailable(self) -> None:
        self._set_status_card("Git", "Unknown", MUTED)
        self._set_status_card("GREEN", "Unknown", MUTED)
        self._set_status_card("Updates", "Unknown", MUTED)
        self._set_status_card("Hygiene", "Unknown", MUTED)
        self._set_status_card("PCC", "Needs adapter", YELLOW)
        self._set_status_card("Runtime", "Unknown", MUTED)
        self._set_status_card("Sync", "Unknown", MUTED)
        lines = [
            f"Repository : {self.root_path}",
            f"Project    : {self.contract.name}",
            f"Type       : {self.contract.kind}",
            "PCC        : Needs standardized machine provider",
            f"Detail     : {self.backend_error or 'No provider detected'}",
        ]
        self.summary_text.configure(state="normal")
        self.summary_text.delete("1.0", "end")
        self.summary_text.insert("1.0", "\n".join(lines))
        self.summary_text.configure(state="disabled")
        self.footer.configure(text="[PCC:Needs Adapter]", fg=YELLOW)
        self.refresh_btn.configure(state="normal")

    def _render_status(self, status: dict[str, Any]) -> None:
        self._last_status = status
        git = status.get("git") or {}
        patches = status.get("patches") or {}
        hygiene = status.get("hygiene") or {}
        binaries = status.get("binaries") or {}
        tools = status.get("tools") or {}

        if not git.get("gitReady"):
            git_text, git_color = "Not ready", RED
        elif git.get("clean"):
            git_text, git_color = "Clean", GREEN
        else:
            changed = int(git.get("staged", 0) or 0) + int(git.get("unstaged", 0) or 0) + int(git.get("untracked", 0) or 0)
            git_text, git_color = (f"Modified ({changed})" if changed else "Modified"), YELLOW
        self._set_status_card("Git", git_text, git_color)

        if git.get("greenMatch"):
            green_text, green_color = "MATCH", GREEN
        elif git.get("greenMarker"):
            green_text, green_color = "STALE", YELLOW
        else:
            green_text, green_color = "NONE", MUTED
        self._set_status_card("GREEN", green_text, green_color)

        invalid = int(patches.get("invalid", 0) or 0)
        pending = int(patches.get("pending", 0) or 0)
        if invalid:
            upd_text, upd_color = f"{invalid} invalid", RED
        elif pending:
            upd_text, upd_color = f"{pending} pending", YELLOW
        else:
            upd_text, upd_color = "0 pending", GREEN
        self._set_status_card("Updates", upd_text, upd_color)

        if hygiene.get("clean", True):
            self._set_status_card("Hygiene", "Clean", GREEN)
        else:
            self._set_status_card("Hygiene", f"{hygiene.get('violationCount', '?')} issue(s)", YELLOW)

        self._set_status_card("PCC", "Ready", GREEN)
        runtime_ready = bool(binaries.get("gui"))
        self._set_status_card("Runtime", "Ready" if runtime_ready else "Not built", GREEN if runtime_ready else YELLOW)

        ahead = git.get("ahead")
        behind = git.get("behind")
        if ahead is None or behind is None:
            sync = "Unknown"
            sync_color = MUTED
        elif int(ahead) == 0 and int(behind) == 0:
            sync = "MATCH"
            sync_color = GREEN
        elif int(behind) > 0:
            sync = f"{ahead}↑ {behind}↓"
            sync_color = RED
        else:
            sync = f"{ahead} ahead"
            sync_color = YELLOW
        self._set_status_card("Sync", sync, sync_color)

        provider = self.backend.provider_label if self.backend is not None else "Unavailable"
        toolchain = str(status.get("toolchain") or "").strip()
        if not toolchain:
            ready_tools = [name for name, ready in tools.items() if ready]
            toolchain = ", ".join(ready_tools) if ready_tools else "Not reported"
        lines = [
            f"Repository : {self.root_path}",
            f"Project    : {self.contract.name}",
            f"Branch     : {git.get('branch') or '<none>'} @ {git.get('headShort') or '<unborn>'}",
            f"Git        : {git_text}",
            f"Sync       : {sync}",
            f"GREEN      : {green_text}",
            f"Updates    : {upd_text}",
            f"Hygiene    : {'Clean' if hygiene.get('clean', True) else 'Needs attention'}",
            f"PCC        : {provider}",
            f"Toolchain  : {toolchain}",
            f"Runtime    : {binaries.get('gui') or 'Not built / not reported'}",
            f"Active log : {(status.get('session') or {}).get('log') or '<not reported>'}",
        ]
        self.summary_text.configure(state="normal")
        self.summary_text.delete("1.0", "end")
        self.summary_text.insert("1.0", "\n".join(lines))
        self.summary_text.configure(state="disabled")

        self.footer.configure(
            text="[" + "] [".join([f"Git:{git_text}", f"GREEN:{green_text}", f"Updates:{upd_text}", f"Hygiene:{'Clean' if hygiene.get('clean', True) else 'WARN'}"]) + "]",
            fg=CYAN,
        )
        self.refresh_btn.configure(state="normal")

    def _refresh_command_catalog(self) -> None:
        self._command_rows = command_catalog(self.contract)
        if hasattr(self, "commands_category_box"):
            categories = ["All", *sorted({row.category for row in self._command_rows if row.category})]
            self.commands_category_box.configure(values=categories)
            if self.commands_category_var.get() not in categories:
                self.commands_category_var.set("All")
        self._reload_registered_commands()
        self._reload_category_command_surfaces()

    def _reload_registered_commands(self) -> None:
        if not hasattr(self, "commands_tree"):
            return
        for iid in self.commands_tree.get_children():
            self.commands_tree.delete(iid)
        query = self.commands_filter_var.get().strip().casefold() if hasattr(self, "commands_filter_var") else ""
        category = self.commands_category_var.get().strip() if hasattr(self, "commands_category_var") else "All"
        for item in self._command_rows:
            if category and category != "All" and item.category != category:
                continue
            haystack = " ".join((item.category, item.key, item.label, item.risk, item.source, item.program)).casefold()
            if query and query not in haystack:
                continue
            self.commands_tree.insert(
                "",
                "end",
                values=(item.category, item.key, item.label, item.risk, item.source, item.program),
            )

    def _selected_registered_command(self) -> SurfaceCommand | None:
        if not hasattr(self, "commands_tree"):
            return None
        selection = self.commands_tree.selection()
        if not selection:
            return None
        values = self.commands_tree.item(selection[0], "values")
        if len(values) < 5:
            return None
        key = str(values[1])
        source = str(values[4])
        return next((row for row in self._command_rows if row.key == key and row.source == source), None)

    def _run_selected_registered_command(self) -> None:
        row = self._selected_registered_command()
        if row is None:
            self._popup("Command Registry", "Select a command first.", kind="warning")
            return
        self._start_registered_command(row)

    def _find_registered_command(self, token: str) -> SurfaceCommand | None:
        needle = token.strip().casefold()
        if not needle:
            return None
        exact = [row for row in self._command_rows if row.key.casefold() == needle]
        if len(exact) == 1:
            return exact[0]
        labels = [row for row in self._command_rows if row.label.casefold() == needle]
        if len(labels) == 1:
            return labels[0]
        return None

    def _start_registered_command(self, row: SurfaceCommand) -> None:
        if row.source == "project_contract":
            self._start_contract_command(row.key, label=row.label)
        else:
            self._start_command(row.key, label=row.label)

    # ------------------------------------------------------------------
    # Operations
    # ------------------------------------------------------------------
    def _start_streaming_process(
        self,
        command_token: str,
        label: str,
        factory: Callable[[], subprocess.Popen[str]],
        *,
        process_note: str = "[ProcessHost] Embedded capture ON / hidden inherited console + universal repo hygiene.",
        focus_tab: str | None = "Project Workspace",
        event_channel: str = "console",
    ) -> None:
        if self._busy or (self._active_proc and self._active_proc.poll() is None):
            self._popup("Project Control Center", "Another PCC job is already running.", kind="warning")
            return
        if focus_tab:
            self._show_app_tab(focus_tab)
        if hasattr(self, "console_text"):
            self.console_text.see("end")
        if focus_tab == "Project Workspace" and hasattr(self, "console_entry"):
            self.console_entry.focus_set()
        self._busy = True
        self._active_command = label
        self.operation_label.configure(text=f"Running: {self._active_command}", fg=CYAN)
        self.console_job_label.configure(text=f"Running: {self._active_command}", fg=CYAN)
        self.stop_btn.configure(state="normal")
        if hasattr(self, "chat_stop_btn"):
            self.chat_stop_btn.configure(state="normal")
        if hasattr(self, "chat_send_btn"):
            self.chat_send_btn.configure(state="disabled")
        if not self.stop_btn.winfo_ismapped():
            self.stop_btn.pack(fill="x", padx=10, pady=(3, 8))
        self.refresh_btn.configure(state="disabled")
        self._append_log(f"\n=== {datetime.now().strftime('%H:%M:%S')} START {self._active_command} ===\n", "info")
        self._append_log(process_note + "\n", "info")
        self.footer.configure(text=f"[Job:Running] [{self._active_command}]", fg=CYAN)

        def work() -> None:
            try:
                proc = factory()
                self._active_proc = proc
                assert proc.stdout is not None
                for line in proc.stdout:
                    self._event_q.put(("log", line))
                    if event_channel == "chat":
                        self._event_q.put(("chat-log", line))
                rc = proc.wait()
                if event_channel == "chat":
                    self._event_q.put(("chat-finalize", (command_token, rc)))
                self._event_q.put(("done", (command_token, rc)))
            except Exception as exc:
                self._event_q.put(("command-error", (command_token, str(exc))))

        threading.Thread(target=work, daemon=True).start()

    def _start_command(self, command: str, extra: Sequence[str] = (), *, label: str | None = None) -> None:
        backend = self.backend
        if backend is None:
            self._popup("Project Control Center", "The universal PCC scan could not bind an executable operation provider for this project.", kind="warning")
            return
        if not backend.supports(command):
            self._popup(
                "Operation Not Available",
                f"The selected project does not expose an operation mapped to '{command}'.\n\nUse Command Registry to review what the project scanner discovered.",
                kind="warning",
            )
            return
        self._start_streaming_process(
            command,
            label or command,
            lambda: backend.popen(command, extra),
        )

    def _start_contract_command(self, command: str, *, label: str | None = None) -> None:
        backend = self.backend
        if backend is None:
            self._popup("Project Control Center", "No project backend is bound.", kind="warning")
            return
        self._start_streaming_process(
            command,
            label or command,
            lambda: backend.popen_contract(command),
            process_note="[ProcessHost] Registered project-contract command / shared dependency environment / live capture.",
        )

    def _start_shell_command(self, command: str) -> None:
        backend = self.backend
        if backend is None:
            self._popup("Project Console", "No project backend is bound.", kind="warning")
            return
        command = command.strip()
        if not command:
            return
        self._start_streaming_process(
            "manual-shell",
            f"CLI: {command}",
            lambda: backend.popen_shell(command),
            process_note="[Manual CLI] User-directed project shell command. Output is live, but this command is not GREEN certification evidence by itself.",
        )

    def _cortex_runtime_root(self) -> Path:
        """Return the Cortex installation/source authority independent of active project.

        The GUI lives under <cortex-root>/tools/control.  Project switching must never make
        Cargo search the selected project for the `cortex` package; the selected project is
        passed only as the workspace target.
        """
        configured = str(os.environ.get("CORTEX_RUNTIME_ROOT") or "").strip()
        if configured:
            candidate = Path(configured).expanduser().resolve()
            if (candidate / "Cargo.toml").is_file():
                return candidate
        return Path(__file__).resolve().parents[2]

    def _cortex_conversation_id(self) -> str:
        key = str(self.root_path).casefold() if os.name == "nt" else str(self.root_path)
        value = self._cortex_conversations.get(key)
        if not value:
            value = uuid.uuid4().hex
            self._cortex_conversations[key] = value
        return value

    def _new_cortex_conversation(self) -> None:
        key = self._chat_project_key()
        self._cortex_conversations[key] = uuid.uuid4().hex
        self._append_log(
            f"[Cortex] New conversation started for {self.contract.name}. Previous conversation history remains archived.\n",
            "info",
        )
        self._render_current_chat()
        self._refresh_chat_history()
        if hasattr(self, "chat_input"):
            self.chat_input.focus_set()


    def _chat_project_key(self) -> str:
        return str(self.root_path).casefold() if os.name == "nt" else str(self.root_path)

    def _refresh_chat_header(self) -> None:
        if hasattr(self, "chat_project_label"):
            self.chat_project_label.configure(
                text=f"{self.contract.name}  •  {compact_path(self.root_path, 96)}  •  conversation {self._cortex_conversation_id()[:10]}"
            )

    def _append_chat_message(self, role: str, text: str) -> None:
        widget = getattr(self, "chat_text", None)
        if widget is None:
            return
        clean = text.strip()
        if not clean:
            return
        widget.configure(state="normal")
        if role == "user":
            widget.insert("end", "YOU\n", "chat-user-head")
            widget.insert("end", clean + "\n", "chat-body")
        elif role == "assistant":
            widget.insert("end", "CORTEX\n", "chat-assistant-head")
            widget.insert("end", clean + "\n", "chat-body")
        else:
            widget.insert("end", "SYSTEM\n", "chat-system-head")
            widget.insert("end", clean + "\n", "chat-muted")
        widget.see("end")
        widget.configure(state="disabled")

    def _clear_chat_transcript(self) -> None:
        widget = getattr(self, "chat_text", None)
        if widget is None:
            return
        widget.configure(state="normal")
        widget.delete("1.0", "end")
        widget.configure(state="disabled")

    def _render_current_chat(self) -> None:
        if not hasattr(self, "chat_text"):
            return
        self._clear_chat_transcript()
        conversation_id = self._cortex_conversation_id()
        cortex_root = self._cortex_runtime_root()
        rows = chat_load_messages(cortex_root, self.root_path, conversation_id)
        if not rows:
            self._append_chat_message(
                "system",
                f"Cortex is attached to {self.contract.name}. Python chat/inspect/plan are available immediately; Apply/Repair use the transactional worker.",
            )
        else:
            for row in rows:
                self._append_chat_message(str(row.get("role") or "assistant"), str(row.get("content") or ""))
        self._chat_rendered_conversation = conversation_id
        self._refresh_chat_header()

    def _refresh_chat_history(self) -> None:
        tree = getattr(self, "chat_history_tree", None)
        if tree is None:
            return
        self._chat_conversation_rows.clear()
        for iid in tree.get_children():
            tree.delete(iid)
        cortex_root = self._cortex_runtime_root()
        current = self._cortex_conversation_id()
        rows = chat_list_conversations(cortex_root, self.root_path)
        present = False
        for index, row in enumerate(rows):
            cid = str(row.get("conversationId") or "")
            if not cid:
                continue
            iid = f"chat-{index}"
            tree.insert("", "end", iid=iid, values=(row.get("title") or "Conversation", row.get("messageCount") or 0))
            self._chat_conversation_rows[iid] = cid
            if cid == current:
                present = True
                tree.selection_set(iid)
        if not present:
            iid = "chat-current"
            tree.insert("", 0, iid=iid, values=("New conversation", 0))
            self._chat_conversation_rows[iid] = current
            tree.selection_set(iid)

    def _chat_history_selected(self, _event: Any = None) -> None:
        tree = getattr(self, "chat_history_tree", None)
        if tree is None:
            return
        selection = tree.selection()
        if not selection:
            return
        conversation_id = self._chat_conversation_rows.get(selection[0])
        if not conversation_id:
            return
        self._cortex_conversations[self._chat_project_key()] = conversation_id
        self._render_current_chat()

    def _chat_insert_newline(self, _event: Any = None) -> str:
        if hasattr(self, "chat_input"):
            self.chat_input.insert("insert", "\n")
        return "break"

    def _submit_chat_input(self, _event: Any = None) -> str:
        if not hasattr(self, "chat_input"):
            return "break"
        prompt = self.chat_input.get("1.0", "end-1c").strip()
        if not prompt:
            return "break"
        mode = str(self.chat_mode_var.get() or "Chat").strip().casefold()
        mode = mode if mode in {"chat", "inspect", "plan", "apply", "repair"} else "chat"
        self.chat_input.delete("1.0", "end")
        self._append_chat_message("user", prompt)
        self._start_cortex_cli(mode, prompt, surface="chat")
        return "break"

    def _start_repair_plan(self) -> None:
        backend = self.backend
        if backend is None:
            self._popup("Repair Coordinator", "No project backend is bound.", kind="warning")
            return
        self._append_chat_message("system", "Reading the latest authoritative PCC failure and building a non-mutating repair plan.")
        self._start_streaming_process(
            "repair-plan",
            "Repair Current Failure — Plan",
            lambda: backend.popen("repair-plan"),
            process_note="[RepairCoordinator] Read-only BuildDoctor → repair-scope plan.",
            focus_tab="Chat",
        )

    def _start_repair_current_full(self) -> None:
        backend = self.backend
        if backend is None:
            self._popup("Repair Coordinator", "No project backend is bound.", kind="warning")
            return
        if not self._popup(
            "Repair Current Failure + FULL",
            "Allow Cortex to repair the currently classified PCC failure inside one bounded source transaction, validate it, run QUICK, run FULL, and automatically roll back if certification fails?",
            kind="warning",
            confirm=True,
        ):
            return
        self._append_chat_message(
            "system",
            "Repair Coordinator started. Source changes remain transactional and will roll back automatically unless targeted validation, QUICK, and FULL all pass.",
        )
        self._start_streaming_process(
            "repair-current-full",
            "Repair Current Failure + FULL",
            lambda: backend.popen("repair-current-full", ["--yes"]),
            process_note="[RepairCoordinator] BuildDoctor → transactional repair → targeted validation → QUICK → FULL → commit/rollback.",
            focus_tab="Chat",
        )

    def _start_cortex_worker_build(self) -> None:
        backend = self.backend
        if backend is None:
            self._popup("Cortex Worker", "No project backend is bound.", kind="warning")
            return
        cortex_root = self._cortex_runtime_root()
        script = cortex_root / "tools" / "control" / "PCCCortexWorker.py"
        if not script.is_file():
            self._popup("Cortex Worker", f"Worker bootstrap is missing:\n{script}", kind="error")
            return
        argv = [sys.executable, str(script), "build", "--root", str(cortex_root)]
        if hasattr(self, "chat_worker_label"):
            self.chat_worker_label.configure(text="Building transactional worker...", fg=CYAN)
        self._append_chat_message("system", "Building the transactional Cortex worker through the bounded ToolchainBroker. Build output is mirrored to Project Console.")
        self._start_streaming_process(
            "cortex-worker-build",
            "Build Cortex Worker",
            lambda: backend.popen_argv(argv, cwd=cortex_root, environment_root=cortex_root),
            process_note="[Cortex] Building only the transactional Cortex worker through the bounded ToolchainBroker.",
            focus_tab="Chat",
        )

    def _finalize_chat_output(self, command: str, rc: int) -> None:
        raw = "".join(self._chat_active_buffer).strip()
        self._chat_active_buffer = []
        lines = raw.splitlines()
        answer_lines: list[str] = []
        for line in lines:
            if line.startswith("[Cortex] CORTEX-PY-BRIDGE-"):
                if hasattr(self, "chat_worker_label"):
                    if "worker=unavailable" in line:
                        self.chat_worker_label.configure(text="Python fallback active", fg=YELLOW)
                    else:
                        self.chat_worker_label.configure(text="Transactional worker active", fg=GREEN)
                continue
            answer_lines.append(line)
        answer = "\n".join(answer_lines).strip()
        if answer:
            self._append_chat_message("assistant" if rc == 0 else "system", answer)
        elif rc != 0:
            self._append_chat_message("system", f"Cortex request failed with exit code {rc}. See Project Console for raw output.")
        self._refresh_chat_history()
        self._refresh_chat_header()


    def _start_cortex_cli(self, mode: str, prompt: str, *, surface: str = "console") -> None:
        """Route Cortex requests through the Python bootstrap broker.

        Chat must not require compiling Cortex.  The broker prefers an existing
        compiled Cortex worker for the full transactional agent surface and can
        fall back to the configured local model provider for ordinary chat.
        """
        backend = self.backend
        if backend is None:
            self._popup("Project Console", "No project backend is bound.", kind="warning")
            return
        prompt = prompt.strip()
        if not prompt:
            self._append_log("[WARN] Cortex prompt is empty.\n", "warn")
            return
        mode = mode if mode in {"chat", "inspect", "plan", "apply", "repair"} else "chat"
        cortex_root = self._cortex_runtime_root()
        bridge = cortex_root / "tools" / "control" / "CortexPythonBridge.py"
        if not bridge.is_file():
            self._append_log(f"[FAIL] Cortex Python bridge was not found: {bridge}\n", "fail")
            return
        argv = [
            sys.executable,
            str(bridge),
            "--cortex-root", str(cortex_root),
            "--workspace", str(self.root_path),
            "--conversation-id", self._cortex_conversation_id(),
            "--owner-pid", str(os.getpid()),
            mode,
            prompt,
        ]
        if surface == "chat":
            self._chat_active_buffer = []
        self._start_streaming_process(
            f"cortex-{mode}",
            f"Cortex {mode}",
            lambda: backend.popen_argv(argv, cwd=cortex_root, environment_root=cortex_root),
            process_note="[Cortex] Python control-plane broker / direct prebuilt Cortex worker / local-provider chat fallback / live output.",
            focus_tab="Chat" if surface == "chat" else "Project Workspace",
            event_channel="chat" if surface == "chat" else "console",
        )

    def _submit_console_input(self, _event: Any = None) -> str:
        if not hasattr(self, "console_input_var"):
            return "break"
        raw = self.console_input_var.get().strip()
        if not raw:
            return "break"
        self.console_input_var.set("")
        self._console_history_index = None
        if not self._console_history or self._console_history[-1] != raw:
            self._console_history.append(raw)
            self._console_history = self._console_history[-100:]
        self._append_log(f"> {raw}\n", "info")
        self._dispatch_console_input(raw)
        return "break"

    def _dispatch_console_input(self, raw: str) -> None:
        text = raw.strip()
        lower = text.casefold()
        if lower in {"/help", "help"}:
            self._append_log(
                "[INFO] Console: command key/label runs registered project operations; "
                "/cortex, /inspect, /plan, /apply, /repair send Cortex requests; "
                "!<shell command> runs the project shell; /newchat starts a clean Cortex conversation; /history, /stop and /clear are local controls.\n",
                "info",
            )
            return
        if lower in {"/clear", "clear"}:
            self._clear_log()
            return
        if lower in {"/newchat", "newchat", "/new-chat", "new-chat"}:
            self._new_cortex_conversation()
            return
        if lower in {"/stop", "stop"}:
            self._stop_active()
            return
        if lower in {"/history", "history"}:
            if not self._console_history:
                self._append_log("[INFO] Console history is empty.\n", "info")
            else:
                for index, item in enumerate(self._console_history, 1):
                    self._append_log(f"{index:>3}: {item}\n", "muted")
            return
        for prefix, mode in (
            ("/cortex ", "chat"),
            ("cortex ", "chat"),
            ("/inspect ", "inspect"),
            ("/plan ", "plan"),
            ("/apply ", "apply"),
            ("/repair ", "repair"),
        ):
            if lower.startswith(prefix):
                self._start_cortex_cli(mode, text[len(prefix):])
                return
        if lower in {"!", "!command"}:
            self._append_log("[INFO] Shell syntax: !<command>  Example: !git status\n", "info")
            return
        if text.startswith("!"):
            self._start_shell_command(text[1:])
            return
        if lower.startswith("shell "):
            self._start_shell_command(text[6:])
            return
        if lower.startswith("op "):
            row = self._find_registered_command(text[3:])
            if row is None:
                self._append_log(f"[FAIL] Unknown or ambiguous project operation: {text[3:]}\n", "fail")
            else:
                self._start_registered_command(row)
            return
        row = self._find_registered_command(text)
        if row is not None:
            self._start_registered_command(row)
            return
        # Obvious build/gate repair requests enter the deterministic read-only
        # Repair Coordinator plan first.  Source mutation still requires the
        # explicit Repair + FULL confirmation path.
        if re.search(r"\b(fix|repair|correct)\b.*\b(build|full[ -]?gate|quick[ -]?gate|pcc|compile|test)\b", lower):
            self._append_chat_message(
                "system",
                "Build/gate repair request detected. Cortex will classify the current failure first; source mutation still requires explicit Repair + FULL approval.",
            )
            self._start_repair_plan()
            return
        # Dotted identifiers are project-operation keys, not natural-language chat.
        # Fail closed when the active project does not expose one instead of sending
        # e.g. `fmt.apply` to Cortex chat and accidentally executing Cortex from the
        # selected project's Cargo workspace.
        if re.fullmatch(r"[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+", text):
            self._append_log(
                f"[FAIL] Project operation is not available for {self.root_path.name}: {text}\n"
                "[INFO] Use Command Registry for this project's operations or prefix natural-language requests with /cortex.\n",
                "fail",
            )
            return
        self._start_cortex_cli("chat", text)

    def _console_history_up(self, _event: Any = None) -> str:
        if not self._console_history:
            return "break"
        if self._console_history_index is None:
            self._console_history_index = len(self._console_history) - 1
        else:
            self._console_history_index = max(0, self._console_history_index - 1)
        self.console_input_var.set(self._console_history[self._console_history_index])
        self.console_entry.icursor("end")
        return "break"

    def _console_history_down(self, _event: Any = None) -> str:
        if self._console_history_index is None:
            return "break"
        if self._console_history_index >= len(self._console_history) - 1:
            self._console_history_index = None
            self.console_input_var.set("")
        else:
            self._console_history_index += 1
            self.console_input_var.set(self._console_history[self._console_history_index])
        self.console_entry.icursor("end")
        return "break"

    def _stop_active(self) -> None:
        proc = self._active_proc
        if proc and proc.poll() is None:
            self.operation_label.configure(text=f"Stopping: {self._active_command}", fg=YELLOW)
            self.console_job_label.configure(text=f"Stopping: {self._active_command}", fg=YELLOW)
            terminate_process_tree(proc)
            self._append_log("Cancellation requested by operator.\n", "warn")

    def _drain_events(self) -> None:
        try:
            while True:
                kind, payload = self._event_q.get_nowait()
                if kind == "log":
                    line = str(payload)
                    up = line.upper()
                    tag = "fail" if ("FAIL" in up or "ERROR" in up) else ("warn" if "WARN" in up else ("pass" if "PASS" in up or "GREEN" in up else ""))
                    self._append_stream_log(line, tag)
                elif kind == "chat-log":
                    self._chat_active_buffer.append(str(payload))
                elif kind == "chat-finalize":
                    command, rc = payload
                    self._finalize_chat_output(str(command), int(rc))
                elif kind == "done":
                    command, rc = payload
                    self._active_proc = None
                    self._busy = False
                    self.stop_btn.configure(state="disabled")
                    self.stop_btn.pack_forget()
                    if hasattr(self, "chat_stop_btn"):
                        self.chat_stop_btn.configure(state="disabled")
                    if hasattr(self, "chat_send_btn"):
                        self.chat_send_btn.configure(state="normal")
                    color = GREEN if rc == 0 else RED
                    state = "PASS" if rc == 0 else f"FAIL ({rc})"
                    self.operation_label.configure(text=f"Last: {command} {state}", fg=color)
                    self.console_job_label.configure(text=f"Last: {command} {state}", fg=color)
                    if command == "cortex-worker-build" and hasattr(self, "chat_worker_label"):
                        self.chat_worker_label.configure(
                            text="Transactional worker ready" if rc == 0 else "Worker build blocked/failed",
                            fg=GREEN if rc == 0 else RED,
                        )
                        self._append_chat_message(
                            "system",
                            "Transactional Cortex worker is ready. Apply/Repair can now use the real mutation path."
                            if rc == 0
                            else "Transactional worker build did not complete. See Project Console for the structured blocker/failure.",
                        )
                    if str(command).casefold() in {"repair-plan", "repair-current", "repair-current-full"}:
                        self._append_chat_message(
                            "system",
                            "Repair Coordinator completed successfully. Review the Project Console/repair receipt for certification evidence."
                            if rc == 0
                            else "Repair Coordinator stopped without certification. Any source candidate that failed validation was rolled back; review Project Console for the blocker.",
                        )
                    self._append_log(f"=== END {command}: {state} ===\n", "pass" if rc == 0 else "fail")
                    self.footer.configure(text=f"[Last:{command}] [{state}]", fg=color)
                    if str(command).casefold() == "full" and int(rc) == 0:
                        # The gate just issued a GREEN marker. Do not leave stale pre-gate
                        # state visible while the authoritative readback runs.
                        self._set_status_card("GREEN", "Verifying", CYAN)
                        self._set_status_card("Git", "Refreshing", CYAN)
                        self._set_status_card("Sync", "Refreshing", CYAN)
                    self._refresh_status_async()
                    self.window.after(50, self._refresh_git_identity_panel)
                elif kind == "command-error":
                    command, detail = payload
                    self._active_proc = None
                    self._busy = False
                    self.stop_btn.configure(state="disabled")
                    self.stop_btn.pack_forget()
                    if hasattr(self, "chat_stop_btn"):
                        self.chat_stop_btn.configure(state="disabled")
                    if hasattr(self, "chat_send_btn"):
                        self.chat_send_btn.configure(state="normal")
                    self.operation_label.configure(text=f"Last: {command} FAIL", fg=RED)
                    self.console_job_label.configure(text=f"Last: {command} FAIL", fg=RED)
                    self._append_log(f"ERROR: {detail}\n", "fail")
                    self._popup("PCC Command Failed", detail, kind="error")
                    self._refresh_status_async()
                elif kind == "project-detail":
                    generation, registry_id, detail, color, adapter_text, catalog_text = payload
                    if int(generation) != self._project_probe_generation:
                        continue
                    selected = self._selected_project()
                    if selected is None or selected.registry_id != registry_id:
                        continue
                    self.project_detail.configure(text=str(detail), fg=str(color))
                    if self.projects_tree.exists(registry_id):
                        values = list(self.projects_tree.item(registry_id, "values"))
                        if len(values) >= 5:
                            values[3] = str(adapter_text)
                            values[4] = str(catalog_text)
                            self.projects_tree.item(registry_id, values=values)
                elif kind == "onboard-done":
                    root, summary, mirror = payload
                    mirror_stats = mirror.get("stats") or {}
                    self._append_log(
                        f"[PASS] Project onboarding catalog + Vault mirror complete: {root} "
                        f"({summary.get('files', 0)} cataloged / {mirror_stats.get('files', 0)} mirrored)\n",
                        "pass",
                    )
                    self._refresh_projects()
                    if Path(root).resolve() == self.root_path.resolve():
                        self._vault_render_summary(summary)
                elif kind == "onboard-error":
                    root, detail = payload
                    self._append_log(f"[WARN] Project onboarding catalog incomplete: {root}: {detail}\n", "warn")
                    self._refresh_projects()
                elif kind == "volume-index-loaded":
                    if not self._volume_running:
                        self._volume_refresh_status(payload)
                        if payload.get("state") != "not_started":
                            self._show_volume_inventory()
                elif kind == "volume-progress":
                    if self._volume_running:
                        self._volume_refresh_status(payload)
                        now = time.monotonic()
                        if now - self._volume_progress_last_query >= 3.0:
                            self._volume_progress_last_query = now
                            self._show_volume_inventory()
                elif kind == "volume-done":
                    self._vault_busy = False
                    self._volume_running = False
                    self.volume_progress.stop()
                    self._volume_refresh_status(payload)
                    self.volume_start_btn.configure(state="normal")
                    self.volume_cancel_btn.configure(state="disabled")
                    self._show_volume_inventory()
                    partial = bool(payload.get("partial"))
                    self._append_log(
                        f"[{'WARN' if partial else 'PASS'}] Persistent drive inventory: "
                        f"{payload.get('state')} · {int(payload.get('entries') or 0):,} entries · "
                        f"{int(payload.get('errors') or 0):,} errors · "
                        f"{int(payload.get('pending_directories') or 0):,} directories pending. "
                        "Source and Vault catalog unchanged.\n", "warn" if partial else "pass")
                elif kind == "volume-error":
                    self._vault_busy = False
                    self._volume_running = False
                    self.volume_progress.stop()
                    self.volume_start_btn.configure(state="normal")
                    self.volume_cancel_btn.configure(state="disabled")
                    self.volume_status.configure(text="Inventory stopped · committed batches remain resumable", fg=RED)
                    self._append_log(f"[FAIL] Persistent drive inventory: {payload}\n", "fail")
                    self._popup("Volume Inventory", str(payload), kind="error")
                elif kind == "volume-view-done":
                    generation, view, report = payload
                    self._volume_filter_busy = False
                    if self._volume_filter_pending:
                        self._volume_filter_pending = False
                        self._show_volume_inventory()
                        continue
                    if int(generation) != self._volume_view_generation:
                        continue
                    self._volume_refresh_status(report)
                    rows = view["rows"]
                    self.volume_tree.delete(*self.volume_tree.get_children())
                    self._volume_rows.clear()
                    for index, entry in enumerate(rows):
                        iid = f"volume:{index}"
                        self._volume_rows[iid] = entry
                        size = (self._human_bytes(int(entry["size_bytes"]))
                                if entry.get("size_bytes") is not None else "—")
                        self.volume_tree.insert("", "end", iid=iid,
                            values=(entry.get("path", ""), entry.get("type", ""), size))
                    total = int(view["total"])
                    current_page = int(view["page"])
                    if not rows and current_page > 0:
                        self._volume_page = 0
                        self._show_volume_inventory()
                        continue
                    self.volume_prev_btn.configure(state="normal" if current_page else "disabled")
                    self.volume_next_btn.configure(state="normal" if view["has_next"] else "disabled")
                    pages = max(1, (total + self._volume_page_size - 1) // self._volume_page_size)
                    self.volume_page_label.configure(text=f"Page {current_page + 1:,} / {pages:,}")
                    self.volume_results_status.configure(
                        text=f"{total:,} matching paths · {len(rows):,} on this page · "
                             f"{int(report.get('entries') or 0):,} indexed", fg=MUTED)
                    state = report.get("state", "not_started")
                    self._volume_set_detail(
                        f"Volume: {report.get('root')}\nIdentity: {report.get('volume_id', self._volume_id)}\n"
                        f"Index: {self._volume_db_path}\n\nState: {state}\n"
                        f"Entries: {int(report.get('entries') or 0):,}\n"
                        f"Unprocessed directories: {int(report.get('pending_directories') or 0):,}\n"
                        f"Unreadable locations: {int(report.get('errors') or 0):,}\n\n"
                        "Search works during scanning. Select a row for metadata and safe path actions.\n"
                        "Source files and the existing Vault catalog are not modified.")
                elif kind == "volume-view-error":
                    generation, detail = payload
                    self._volume_filter_busy = False
                    if self._volume_filter_pending:
                        self._volume_filter_pending = False
                        self._show_volume_inventory()
                        continue
                    if int(generation) == self._volume_view_generation:
                        self.volume_results_status.configure(text=f"Index query failed: {detail}", fg=RED)
                elif kind == "volume-errors-done":
                    if payload:
                        lines = [f"{row['path']} [{row['operation']}]: {row['message']}" for row in payload]
                        self._volume_set_detail("Unreadable inventory locations (first 30):\n\n" + "\n\n".join(lines))
                    else:
                        self._volume_set_detail("No unreadable locations recorded in the current inventory index.")
                elif kind == "volume-errors-error":
                    self._volume_set_detail(f"Could not query access gaps: {payload}")
                elif kind == "vault-progress":
                    files = int((payload or {}).get("files") or 0)
                    hashed = int((payload or {}).get("hashed") or 0)
                    reused = int((payload or {}).get("reusedHashes") or 0)
                    self.vault_scan_status.configure(text=f"Scanning {files} files · {hashed} hashed · {reused} cached", fg=CYAN)
                elif kind == "vault-done":
                    root, summary = payload
                    self._vault_busy = False
                    if Path(root).resolve() == self.root_path.resolve():
                        self.vault_scan_status.configure(text=f"PASS · {summary.get('files', 0)} files · {summary.get('elapsedSeconds', 0)}s", fg=GREEN)
                        self._vault_render_summary(summary)
                        self._vault_refresh_tree()
                    self._append_log(f"[PASS] Vault/Forge catalog scan complete: {root} ({summary.get('files', 0)} files)\n", "pass")
                elif kind == "vault-intake-progress":
                    files = int((payload or {}).get("files") or 0)
                    bytes_value = int((payload or {}).get("bytes") or 0)
                    reused = int((payload or {}).get("reused") or 0)
                    hashed = int((payload or {}).get("hashed") or 0)
                    self.vault_scan_status.configure(
                        text=f"Intake · {files:,} files · {self._human_bytes(bytes_value)} · {hashed:,} hashed · {reused:,} reused",
                        fg=CYAN,
                    )
                elif kind == "vault-intake-done":
                    self._vault_busy = False
                    summary = payload or {}
                    self.vault_scan_status.configure(
                        text=f"Intake PASS · {int(summary.get('files') or 0):,} files · {summary.get('humanBytes', '0 B')}",
                        fg=GREEN,
                    )
                    self._append_log(
                        f"[PASS] Vault Intake audit complete: {summary.get('files', 0)} files / {summary.get('humanBytes', '0 B')} / "
                        f"{summary.get('projectCandidates', 0)} project candidate(s) / {summary.get('unknownFiles', 0)} unknown file(s).\n",
                        "pass",
                    )
                    self._popup(
                        "Vault Intake Audit",
                        f"Inventory complete.\n\nFiles: {int(summary.get('files') or 0):,}\n"
                        f"Size: {summary.get('humanBytes', '0 B')}\n"
                        f"Project candidates: {int(summary.get('projectCandidates') or 0):,}\n"
                        f"Exact duplicate groups: {int(summary.get('exactDuplicateGroups') or 0):,}\n"
                        f"Sampled duplicate candidates: {int(summary.get('sampledDuplicateCandidateGroups') or 0):,}\n"
                        f"Unknown files: {int(summary.get('unknownFiles') or 0):,}\n\n"
                        "Nothing was moved or deleted. Create Chat Handoff to review the classification here.",
                        kind="success",
                    )
                elif kind == "vault-intake-cancelled":
                    self._vault_busy = False
                    self.vault_scan_status.configure(text="Intake audit cancelled", fg=YELLOW)
                    self._append_log(f"[WARN] Vault Intake audit cancelled: {payload}\n", "warn")
                elif kind == "vault-intake-handoff-done":
                    self._vault_busy = False
                    path = Path(str(payload))
                    self.vault_scan_status.configure(text="Intake handoff ready", fg=GREEN)
                    self._append_log(f"[PASS] Vault Intake planning handoff created: {path}\n", "pass")
                    self._popup(
                        "Vault Intake Handoff",
                        f"Planning handoff created. Upload this ZIP here for classification/organization planning:\n\n{path}",
                        kind="success",
                    )
                    try:
                        reveal_file(path)
                    except Exception:
                        pass
                elif kind == "vault-intake-error":
                    self._vault_busy = False
                    self.vault_scan_status.configure(text="Intake audit failed", fg=RED)
                    self._append_log(f"[FAIL] Vault Intake audit: {payload}\n", "fail")
                    self._popup("Vault Intake Audit", str(payload), kind="error")
                elif kind == "vault-mirror-all-progress":
                    index = int((payload or {}).get("index") or 0)
                    total = int((payload or {}).get("total") or 0)
                    root = str((payload or {}).get("root") or "")
                    self.vault_scan_status.configure(text=f"Mirroring project {index}/{total} · {Path(root).name}", fg=CYAN)
                elif kind == "vault-mirror-all-done":
                    self._vault_busy = False
                    total = int((payload or {}).get("total") or 0)
                    failures = list((payload or {}).get("failures") or [])
                    passed = total - len(failures)
                    self.vault_scan_status.configure(text=f"Mirror sweep · {passed}/{total} PASS", fg=GREEN if not failures else YELLOW)
                    self._append_log(f"[{'PASS' if not failures else 'WARN'}] Vault mirror sweep: {passed}/{total} project(s) mirrored\n", "pass" if not failures else "warn")
                    self._refresh_projects()
                    if failures:
                        self._popup("Vault Mirror Sweep", f"Mirrored {passed}/{total} project(s).\n\n" + "\n".join(failures[:8]), kind="warning")
                elif kind == "vault-mirror-progress":
                    files = int((payload or {}).get("files") or 0)
                    new_objects = int((payload or {}).get("newObjects") or 0)
                    reused = int((payload or {}).get("reusedObjects") or 0)
                    self.vault_scan_status.configure(text=f"Mirroring {files} files · {new_objects} new · {reused} reused", fg=CYAN)
                elif kind == "vault-mirror-done":
                    root, result = payload
                    self._vault_busy = False
                    stats = result.get("stats") or {}
                    self.vault_scan_status.configure(text=f"Mirror PASS · {stats.get('files', 0)} files", fg=GREEN)
                    self._append_log(
                        f"[PASS] Vault project mirror: {root} · snapshot {result.get('snapshotId')} · "
                        f"{stats.get('newObjects', 0)} new object(s), {stats.get('reusedObjects', 0)} reused\n",
                        "pass",
                    )
                elif kind == "vault-mirror-error":
                    self._vault_busy = False
                    self.vault_scan_status.configure(text="Mirror failed", fg=RED)
                    self._append_log(f"[FAIL] Vault mirror: {payload}\n", "fail")
                    self._popup("Vault Mirror Failed", str(payload), kind="error")
                elif kind == "vault-error":
                    self._vault_busy = False
                    self.vault_scan_status.configure(text="Scan failed", fg=RED)
                    self._append_log(f"[FAIL] Vault/Forge scan: {payload}\n", "fail")
                    self._popup("Vault Scan Failed", str(payload), kind="error")
                elif kind == "status":
                    generation, status = payload
                    if int(generation) != self._status_generation:
                        continue
                    self._render_status(status)
                elif kind == "status-error":
                    generation, detail = payload
                    if int(generation) != self._status_generation:
                        continue
                    self.refresh_btn.configure(state="normal")
                    self.footer.configure(text="[Status:Unavailable]", fg=RED)
                    self._append_log(f"Status refresh failed: {detail}\n", "fail")
        except queue.Empty:
            pass
        self.window.after(60, self._drain_events)

    def _latest_applied_patch_identity(self) -> tuple[str, str] | None:
        """Return the newest successfully applied patch identity for commit-message carry-forward."""
        receipts = self.root_path / "artifacts" / "patches" / "receipts"
        if not receipts.is_dir():
            return None
        candidates: list[tuple[float, str, str]] = []
        for path in receipts.glob("*.json"):
            try:
                data = json.loads(path.read_text(encoding="utf-8-sig"))
            except Exception:
                continue
            if str(data.get("status") or "").strip().casefold() != "applied":
                continue
            patch_id = str(data.get("patchId") or "").strip()
            if not patch_id:
                continue
            title = str(data.get("title") or "").strip()
            try:
                stamp = path.stat().st_mtime
            except OSError:
                stamp = 0.0
            candidates.append((stamp, patch_id, title))
        if not candidates:
            return None
        _stamp, patch_id, title = max(candidates, key=lambda row: row[0])
        return patch_id, title

    def _green_commit_default(self) -> tuple[str, str]:
        patch = self._latest_applied_patch_identity()
        if patch:
            patch_id, title = patch
            subject = f"{self.contract.name} GREEN {patch_id}"
            if title:
                subject += f" - {title}"
            return subject, f"Current applied patch: {patch_id}" + (f" — {title}" if title else "")
        marker = self.root_path / ".cortex" / "last-green-quality-gate.json"
        if marker.is_file():
            try:
                data = json.loads(marker.read_text(encoding="utf-8-sig"))
                head = str(data.get("gitHead") or "").strip()[:12]
                created = str(data.get("createdUtc") or "").strip()
                basis = "Current certified GREEN source"
                if head:
                    basis += f" @ {head}"
                if created:
                    basis += f" ({created})"
                return f"{self.contract.name} GREEN checkpoint - {datetime.now().strftime('%Y-%m-%d %H:%M')}", basis
            except Exception:
                pass
        return (
            f"{self.contract.name} GREEN checkpoint - {datetime.now().strftime('%Y-%m-%d %H:%M')}",
            "Current certified GREEN source",
        )


    def _ask_commit_message(self, *, push: bool) -> str | None:
        tk = self.tk
        default, basis = self._green_commit_default()
        dialog = tk.Toplevel(self.window)
        dialog.withdraw()
        dialog.configure(bg=BG)
        dialog.overrideredirect(True)
        dialog.transient(self.window)
        dialog.resizable(False, False)

        outer = tk.Frame(dialog, bg=CYAN, padx=1, pady=1)
        outer.pack(fill="both", expand=True)
        shell = tk.Frame(outer, bg=PANEL)
        shell.pack(fill="both", expand=True)

        result: list[str | None] = [None]

        def close(value: str | None) -> None:
            result[0] = value
            try:
                dialog.grab_release()
            except Exception:
                pass
            dialog.destroy()

        titlebar = tk.Frame(shell, bg=PANEL_2, height=44)
        titlebar.pack(fill="x")
        titlebar.pack_propagate(False)
        tk.Label(
            titlebar,
            text="COMMIT + PUSH CERTIFIED GREEN" if push else "COMMIT CERTIFIED GREEN",
            bg=PANEL_2,
            fg=TEXT,
            font=("Segoe UI Semibold", 12),
        ).pack(side="left", padx=16)
        close_btn = tk.Button(
            titlebar,
            text="×",
            command=lambda: close(None),
            bg=PANEL_2,
            fg=MUTED,
            activebackground=RED,
            activeforeground=TEXT,
            bd=0,
            relief="flat",
            cursor="hand2",
            font=("Segoe UI Semibold", 15),
            width=3,
        )
        close_btn.pack(side="right", fill="y")

        header = tk.Frame(shell, bg=PANEL)
        header.pack(fill="x", padx=18, pady=(14, 8))
        tk.Label(
            header,
            text=f"{self.contract.name}  ·  {basis}",
            bg=PANEL,
            fg=GREEN,
            font=("Segoe UI", 9),
            wraplength=700,
            justify="left",
        ).pack(anchor="w")

        git = self._last_status.get("git") or {}
        green_state = "MATCH" if git.get("greenMatch") else ("STALE" if git.get("greenMarker") else "NONE")
        branch = str(git.get("branch") or "unknown")
        ahead = git.get("ahead")
        behind = git.get("behind")
        sync = "unknown" if ahead is None or behind is None else (f"{ahead} ahead / {behind} behind")
        statebar = tk.Frame(shell, bg=PANEL_2)
        statebar.pack(fill="x", padx=18, pady=(0, 10))
        for label, value, color in (
            ("GREEN", green_state, GREEN if green_state == "MATCH" else YELLOW),
            ("Branch", branch, TEXT),
            ("Sync", sync, GREEN if ahead == 0 and behind == 0 else YELLOW),
        ):
            cell = tk.Frame(statebar, bg=PANEL_2)
            cell.pack(side="left", padx=12, pady=7)
            tk.Label(cell, text=label, bg=PANEL_2, fg=MUTED, font=("Segoe UI", 7)).pack(anchor="w")
            tk.Label(cell, text=value, bg=PANEL_2, fg=color, font=("Segoe UI Semibold", 9)).pack(anchor="w")

        body = tk.Frame(shell, bg=PANEL)
        body.pack(fill="both", expand=True, padx=18, pady=(0, 10))
        tk.Label(body, text="Commit message", bg=PANEL, fg=MUTED, font=("Segoe UI Semibold", 9)).pack(anchor="w")
        editor_frame = tk.Frame(body, bg="#07090b", highlightthickness=1, highlightbackground=BORDER)
        editor_frame.pack(fill="both", expand=True, pady=(6, 0))
        editor = tk.Text(
            editor_frame,
            bg="#07090b",
            fg=TEXT,
            insertbackground=TEXT,
            selectbackground="#21404a",
            selectforeground=TEXT,
            bd=0,
            relief="flat",
            font=("Consolas", 10),
            wrap="word",
            undo=True,
            height=12,
        )
        scroll = self._dark_scrollbar(editor_frame, orient="vertical", command=editor.yview)
        editor.configure(yscrollcommand=scroll.set)
        editor.pack(side="left", fill="both", expand=True, padx=(11, 0), pady=10)
        scroll.pack(side="right", fill="y", padx=(5, 8), pady=8)
        editor.insert("1.0", default)
        editor.tag_add("sel", "1.0", "end-1c")

        def accept() -> None:
            message = editor.get("1.0", "end-1c").strip()
            if not message:
                self._popup("Commit Certified GREEN", "Enter a commit message before continuing.", kind="warning", parent=dialog)
                editor.focus_set()
                return
            close(message)

        actions = tk.Frame(shell, bg=PANEL)
        actions.pack(fill="x", padx=18, pady=(0, 16))
        tk.Label(actions, text="Esc = Cancel  ·  Ctrl+Enter = Commit", bg=PANEL, fg=MUTED, font=("Segoe UI", 8)).pack(side="left")
        self._button(actions, "Cancel", lambda: close(None), compact=True).pack(side="right", padx=(8, 0))
        self._button(
            actions,
            "Commit + Push GREEN" if push else "Commit GREEN",
            accept,
            primary=True,
            compact=True,
        ).pack(side="right")

        dialog.bind("<Escape>", lambda _e: close(None))
        dialog.bind("<Control-Return>", lambda _e: accept())
        dialog.protocol("WM_DELETE_WINDOW", lambda: close(None))
        self._center_modal(dialog, 760, 455)
        self._round_window(dialog)
        self._show_owned_modal(dialog, focus_widget=editor)
        self.window.wait_window(dialog)
        return result[0]

    def _commit_green(self) -> None:
        if not self._git_identity_snapshot().get("configured"):
            self._popup("Git Identity Required", "Configure a Git author name and email before committing certified GREEN source.", kind="warning")
            self._configure_git_identity()
            return
        message = self._ask_commit_message(push=False)
        if message:
            self._start_command("commit-green", ["--message", message], label="commit-green")

    def _commit_push_green(self) -> None:
        if not self._git_identity_snapshot().get("configured"):
            self._popup("Git Identity Required", "Configure a Git author name and email before committing and pushing certified GREEN source.", kind="warning")
            self._configure_git_identity()
            return
        message = self._ask_commit_message(push=True)
        if message and self._popup(
            "Commit + Push GREEN",
            "Commit the current certified GREEN source and push it to the configured remote?",
            kind="warning",
            confirm=True,
        ):
            self._start_command("commit-push-green", ["--message", message], label="commit-push-green")

    def _apply_updates(self) -> None:
        if self._popup("Apply Validated Updates", "Apply the currently validated PCC update queue? Invalid updates remain fail-closed.", kind="warning", confirm=True):
            self._start_command("patch-apply", ["--yes"], label="apply-updates")

    def _open_latest_debug(self) -> None:
        path = latest_debug_bundle(self.root_path)
        if path:
            reveal_file(path)
        else:
            open_path(self.root_path / "artifacts" / "debug")

    def _open_cli(self) -> None:
        control = self.contract.raw.get("root_control_center") or {}
        declared = str(control.get("launcher") or "").strip()
        candidates = []
        if declared:
            candidates.append(self.root_path / declared)
        candidates.extend([
            self.root_path / "PROJECT_CONTROL_CENTER.cmd",
            *sorted(self.root_path.glob("*Tools.cmd")),
            *sorted(self.root_path.glob("*ControlCenter.cmd")),
        ])
        launcher = next((path for path in candidates if path.is_file()), None)
        if os.name == "nt" and launcher is not None:
            # Root launchers own their own interactive syntax; do not force Cortex's --cli
            # switch onto legacy/adopted project utilities.
            argv = ["cmd.exe", "/k", str(launcher)]
            if launcher.name.casefold() == "project_control_center.cmd":
                argv.append("--cli")
            subprocess.Popen(argv, cwd=str(self.root_path), creationflags=getattr(subprocess, "CREATE_NEW_CONSOLE", 0))
            return
        self._popup("Project Control Center", "No interactive project launcher was discovered for the selected project.")

    def _on_close(self) -> None:
        if self._active_proc and self._active_proc.poll() is None:
            if not self._popup("Active PCC Job", "A Project Control Center job is still running. Stop it and close?", kind="warning", confirm=True):
                return
            terminate_process_tree(self._active_proc)
        self._vault_cancel = True  # signal the read-only worker only after closing is confirmed
        self.window.destroy()

    def run(self) -> int:
        self.window.mainloop()
        return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Universal Project Control Center GUI")
    p.add_argument("--root")
    p.add_argument("--self-test", action="store_true")
    return p


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = resolve_root(args.root)
    if args.self_test:
        for note in validate_surface(root):
            print(f"PASS {note}")
        registry = ProjectRegistry()
        print(f"PASS registry={registry.path}")
        print(f"PASS registered-projects={len(registry.entries())}")
        print("PASS self-test-registry-mode=read-only")
        try:
            import tkinter as tk
            print(f"PASS tkinter={tk.TkVersion}")
        except Exception as exc:
            raise SurfaceError(f"Tkinter GUI runtime is unavailable: {exc}") from exc
        print(f"PASS gui-version={GUI_VERSION}")
        return 0
    return CortexPCCGui(root).run()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SurfaceError as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
