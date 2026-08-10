# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Top-level UMAMI histogram viewer window."""

import html
import sys
import time
from pathlib import Path

import numpy as np
import pyqtgraph as pg
from pyqtgraph import exporters
from pyqtgraph.Qt.QtCore import QSettings, Qt, QTimer
from pyqtgraph.Qt.QtGui import QKeySequence
from pyqtgraph.Qt.QtWidgets import (
    QApplication,
    QCheckBox,
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QDoubleSpinBox,
    QFileDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QMenu,
    QMessageBox,
    QPushButton,
    QSizePolicy,
    QSpinBox,
    QSplitter,
    QTabWidget,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from . import __version__
from .aux_histo import AuxHistoWindow, discover_aux_histo_output
from .axis_items import StripedRectROI, ZoomViewBox
from .client import UmamiClient
from .events_panel import EventsPanel
from .icons import icon_button, load_icon
from .log_panel import LogPanel
from .logo_widget import LogoBuildupWidget
from .mesy_config import McpdConfigWindow, discover_mesy_inputs
from .params_tree import ParamsTree
from .plot_utils import COLORMAPS, set_image_data
from .projection_windows import DiffractogramWindow, TofSpectrumWindow
from .replay_window import ReplayWindow
from .shm import ShmHistogram
from .status_panel import StatusPanel
from .tof_config import TofConfigWindow, discover_tof_recipes

IMAGE_REFRESH_MS = 250
STATE_POLL_MS = 1000
RATE_SAMPLES = 4  # ~1 sec of history at IMAGE_REFRESH_MS

# height/width of icons/wordmark.svg's viewBox (199.328 x 44.863891) -- used
# to size the About dialog's wordmark without distorting it
WORDMARK_ASPECT = 44.863891 / 199.328

# kept in sync by hand with Cargo.toml's [package.authors]
AUTHORS = [
    'Georg Brandl <g.brandl@fz-juelich.de>',
    'Alexander Zaft <a.zaft@fz-juelich.de>',
    'Enrico Faulhaber <enrico.faulhaber@frm2.tum.de>',
]


class MainWindow(QWidget):
    """Top-level UMAMI histogram viewer.

    Has control buttons, the live 2-D histogram image, per-input status, and a
    docked recipe/output params table. Owns the shm mapping and command-socket
    client, and drives all periodic polling (image refresh, connection/state
    heartbeat).
    """

    def __init__(self, shm_name):
        super().__init__()
        self.shm_name = shm_name
        self.resize(1100, 800)
        self.setWindowTitle('UMAMI histogram')
        self.setLayout(QVBoxLayout())
        self.layout().setContentsMargins(0, 0, 0, 0)

        try:
            self.histo = ShmHistogram(shm_name)
        except RuntimeError as e:
            QMessageBox.critical(None, 'Error', str(e))
            sys.exit(1)

        self.log_panel = LogPanel()
        self.client = UmamiClient(shm_name, self.log_panel)
        # Quick-setup windows are created lazily, on first use -- see
        # `_quick_setup_window()` -- since there's an ever-growing set of
        # them and most instances only ever need a few.
        self.aux_histo_window = self.mcpd_config_window = self.tof_config_window = None
        self.replay_window = None
        self.proj_window = self.t_proj_window = None

        self.current_mode = None
        self.instance_name = None
        self.rate_samples = []  # trailing (time, total) samples, up to RATE_SAMPLES
        self.counter_samples = []  # trailing (time, (events, neutrons, tzero, *mon))
        self.roi_rate_samples = []  # trailing (time, roi_total) samples
        self.last_t = None
        self.last_buf = None
        self.was_connected = False
        self.last_elapsed_s = None
        self.settings = QSettings()

        self._build_main_plot()
        self._build_status_and_mode()
        self._build_buttons()
        self._build_display_controls()
        self._build_dump_controls()
        self._build_params_panel()
        self._build_events_panel()
        self._build_right_tabs()
        self._assemble()
        self._load_settings()
        QApplication.instance().aboutToQuit.connect(self._save_settings)

        self.image_timer = QTimer()
        self.image_timer.timeout.connect(self.update_buffer)
        self.image_timer.start(IMAGE_REFRESH_MS)

        self.state_timer = QTimer()
        self.state_timer.timeout.connect(self.poll_state)
        self.state_timer.start(STATE_POLL_MS)

        # get an initial connection/log entry and populate the mode list &
        # params right away rather than waiting for the first timer tick;
        # mark the connection as already-established first so poll_state()'s
        # reconnect detection doesn't treat this initial setup as a
        # redundant reinit
        self.client.ping()
        self.was_connected = self.client.connected
        modes = self.client.get_modes() or []
        self.mode_combo.addItems(modes)
        self.refresh_params()
        self.poll_state()

    def closeEvent(self, event):  # noqa: N802
        # other top-level windows may still be open (see _quick_setup_window)
        super().closeEvent(event)
        QApplication.instance().quit()

    # ---- main image plot ----

    def _build_main_plot(self):
        self.graphics = pg.GraphicsLayoutWidget()
        self.plot = self.graphics.addPlot(viewBox=ZoomViewBox())
        self.img = pg.ImageItem(border='w', axisOrder='row-major')
        # shift by half a pixel so integer axis ticks land on pixel centers
        # (pixel i spans [i-0.5, i+0.5]) instead of edges -- a plain
        # translation via setPos(), not setRect(), since setRect() derives
        # its scale from the image's current dimensions and computes the
        # wrong scale before any image has been assigned yet
        self.img.setPos(-0.5, -0.5)
        self.plot.addItem(self.img)
        self.img.setColorMap(pg.colormap.get('viridis'))
        self.plot.enableAutoRange('xy', True)
        self.plot.scene().sigMouseMoved.connect(self.on_mouse_moved)

        self.roi_item = StripedRectROI((0, 0), (0, 0), movable=False)
        self.roi_item.addScaleHandle([0, 0], [1, 1])
        self.roi_item.addScaleHandle([0, 1], [1, 0])
        self.roi_item.addScaleHandle([1, 0], [0, 1])
        self.roi_item.setZValue(10)
        self.roi_item.hide()
        self._roi_snap_guard = False  # re-entrancy guard, see _on_roi_region_changed
        self.roi_item.sigRegionChanged.connect(self._on_roi_region_changed)
        self.plot.addItem(self.roi_item)

    # ---- projection plots: separate windows, created lazily on request ----

    def open_projection_window(self):
        window = self._quick_setup_window('proj_window', DiffractogramWindow)
        window.show()
        window.raise_()

    def open_t_projection_window(self):
        window = self._quick_setup_window('t_proj_window', TofSpectrumWindow)
        window.show()
        window.raise_()
        self.update_t_axis_labels()

    def update_t_axis_labels(self):
        if self.t_proj_window is None:
            return
        info = (self.params_tree.params or {}).get(f'{self.current_mode}.time_bins')
        value = info.get('value') if info else None
        self.t_proj_window.set_bin_edges_ns(value)

    def refresh_params(self):
        self.params_tree.refresh()
        self.update_t_axis_labels()
        params = self.params_tree.params or {}
        self.mcpd_config_btn.setVisible(bool(discover_mesy_inputs(params)))
        self.tof_config_btn.setVisible(bool(discover_tof_recipes(params)))
        self.aux_histo_btn.setVisible(discover_aux_histo_output(params) is not None)
        if self.aux_histo_window is not None and self.aux_histo_window.isVisible():
            self.aux_histo_window.refresh()
        if self.mcpd_config_window is not None and self.mcpd_config_window.isVisible():
            self.mcpd_config_window.refresh()
        if self.tof_config_window is not None and self.tof_config_window.isVisible():
            self.tof_config_window.refresh()

    def update_buffer(self):  # noqa: C901, PLR0912, PLR0915
        t = self.t_spin.value()
        if t != self.last_t:
            # switching slices jumps the total to an unrelated bin's count;
            # drop the old baseline so the rate isn't computed across that jump
            self.rate_samples.clear()
            self.roi_rate_samples.clear()
            self.last_t = t
        run_id = self.histo.run_id
        buf = self.histo.read_plane(t)
        # a copy, not the raw mmap view -- keeping a view alive across ticks
        # would block the mmap from ever closing (e.g. on reconnect)
        self.last_buf = buf.copy()
        set_image_data(
            self.img, buf, log=self.log_scale_check.isChecked(),
            auto_levels=self.auto_levels_check.isChecked(),
            level_min=self.level_min_spin.value(),
            level_max=self.level_max_spin.value())
        if self.proj_window is not None:
            x_edges = np.arange(self.histo.nx + 1) - 0.5
            self.proj_window.update_data(x_edges, buf.sum(axis=0))
        if self.t_proj_window is not None:
            edges_ns = self.t_proj_window.bin_edges_ns
            n = len(edges_ns) if edges_ns is not None else self.histo.nt
            # read_time_projection() clamps to self.histo.nt internally, so n
            # must match that clamp too, or the returned y is shorter than
            # the edges array setData() expects.
            n = min(n, self.histo.nt)
            self.t_proj_window.update_data(n, self.histo.read_time_projection(n))

        total = int(buf.sum())
        now = time.monotonic()
        self.rate_samples.append((now, total))
        if len(self.rate_samples) > RATE_SAMPLES:
            self.rate_samples.pop(0)
        rate = None
        if len(self.rate_samples) > 1:
            old_time, old_total = self.rate_samples[0]
            if total >= old_total:
                rate = (total - old_total) / (now - old_time)
        elapsed_s = self._compute_elapsed_s()
        self.status_panel.update_run_info(run_id, elapsed_s=elapsed_s, total=total,
                                          rate=rate)

        roi = self._roi_bounds()
        if roi is not None:
            x0, y0, x1, y1 = roi
            roi_total = int(buf[y0:y1, x0:x1].sum())
        else:
            roi_total = 0
        self.roi_rate_samples.append((now, roi_total))
        if len(self.roi_rate_samples) > RATE_SAMPLES:
            self.roi_rate_samples.pop(0)
        roi_rate = None
        if roi is not None and len(self.roi_rate_samples) > 1:
            old_time, old_total = self.roi_rate_samples[0]
            if roi_total >= old_total:
                roi_rate = (roi_total - old_total) / (now - old_time)

        total_events, total_neutrons, lifetime_ns, tzero_count, monitor_counts = \
            self.histo.read_counters()
        counters = (total_neutrons, total_events, tzero_count, *monitor_counts)
        self.counter_samples.append((now, counters))
        if len(self.counter_samples) > RATE_SAMPLES:
            self.counter_samples.pop(0)
        counter_rates = [None] * len(counters)
        if len(self.counter_samples) > 1:
            old_time, old_counters = self.counter_samples[0]
            for i, (cur, old) in enumerate(zip(counters, old_counters)):
                if cur >= old:
                    counter_rates[i] = (cur - old) / (now - old_time)
        self.events_panel.update_counts(
            total, roi_total, total_neutrons, total_events, tzero_count,
            monitor_counts, lifetime_ns, (rate, roi_rate, *counter_rates))

    def _compute_elapsed_s(self):
        """Seconds since the last StartOfRun, frozen once the run has ended.

        Only advances from the live wall clock while shm reports a run as
        active; once it ends, keeps returning the last value computed while
        running. If this session never observed the run while it was
        active (e.g. the GUI was started after it ended), there's no
        last-known value to freeze at, so this reports unknown (None).
        """
        run_start = self.histo.run_start
        if not run_start:
            self.last_elapsed_s = None
            return None
        if self.histo.running:
            self.last_elapsed_s = max(0, int(time.time() - run_start))
        return self.last_elapsed_s

    def reopen_histogram(self):
        """Re-attach to the shm segment from scratch.

        After a reconnect, umami may have restarted with different histogram
        dimensions, and the old mapping would otherwise keep showing stale,
        frozen data.
        """
        try:
            new_histo = ShmHistogram(self.shm_name)
        except RuntimeError as e:
            self.log_panel.error(f'Could not reopen shared memory: {e}')
            return
        self.histo = new_histo
        self.rate_samples.clear()
        self.counter_samples.clear()
        self.roi_rate_samples.clear()
        self.plot.enableAutoRange('xy', True)
        self.t_spin.setRange(0, max(self.histo.nt - 1, 0))

    # ---- status/state polling (also serves as the connection heartbeat) ----

    def _build_status_and_mode(self):
        self.status_panel = StatusPanel()
        self.mode_combo = QComboBox()
        self.mode_combo.activated.connect(self.on_mode_selected)

    def on_mode_selected(self, index):
        self.client.set_mode(self.mode_combo.itemText(index))

    def sync_mode_combo(self, mode_name):
        if mode_name and self.mode_combo.currentText() != mode_name:
            self.mode_combo.blockSignals(True)
            index = self.mode_combo.findText(mode_name)
            if index >= 0:
                self.mode_combo.setCurrentIndex(index)
            self.mode_combo.blockSignals(False)

    def reinit_after_reconnect(self):
        """Re-pull everything instead of trusting stale state.

        The pipeline may have (re)started with a different config across
        a disconnect.
        """
        self.log_panel.info('Reconnected -- reinitializing (config may have changed)')
        self.reopen_histogram()
        if self.aux_histo_window is not None:
            self.aux_histo_window.invalidate_all()
        self.status_panel.reset_inputs()
        modes = self.client.get_modes()
        if modes is not None:
            self.mode_combo.blockSignals(True)
            self.mode_combo.clear()
            self.mode_combo.addItems(modes)
            self.mode_combo.blockSignals(False)
        self.refresh_params()

    def poll_state(self):
        state = self.client.get_state()
        self.status_panel.set_connected(self.client.connected)
        if self.client.connected and not self.was_connected:
            self.reinit_after_reconnect()
        self.was_connected = self.client.connected
        if state is not None:
            self.status_panel.update_state(state)
            self.sync_mode_combo(state.get('mode'))
            new_mode = state.get('mode')
            if new_mode and new_mode != self.current_mode:
                self.current_mode = new_mode
                self.update_t_axis_labels()
            new_name = state.get('name')
            if new_name != self.instance_name:
                self.instance_name = new_name
                title = (f'UMAMI histogram — {self.instance_name}'
                         if self.instance_name else 'UMAMI histogram')
                self.setWindowTitle(title)

    # ---- controls ----

    def _build_buttons(self):
        frame = QFrame()
        frame.setLayout(QHBoxLayout())
        frame.layout().setContentsMargins(8, 5, 8, 5)
        frame.setSizePolicy(QSizePolicy.Policy.Preferred,
                            QSizePolicy.Policy.Fixed)

        btn = icon_button('reset', 'Reset')
        btn.clicked.connect(self.client.reset)
        frame.layout().addWidget(btn)

        frame.layout().addWidget(QLabel('Run ID:'))

        self.run_id_field = QLineEdit()
        self.run_id_field.setPlaceholderText('<use current time>')
        self.run_id_field.setMaximumWidth(160)
        frame.layout().addWidget(self.run_id_field)

        btn = icon_button('clear', 'Clear', tint=False)
        btn.setStyleSheet('background-color: rgb(190, 190, 190); color: black;')
        btn.clicked.connect(self.client.clear)
        frame.layout().addWidget(btn)

        btn = icon_button('start', 'Start', tint=False)
        btn.setStyleSheet('background-color: rgb(140, 205, 140); color: black;')
        btn.clicked.connect(self.on_start_clicked)
        frame.layout().addWidget(btn)

        btn = icon_button('stop', 'Stop', tint=False)
        btn.setStyleSheet('background-color: rgb(255, 150, 150); color: black;')
        btn.clicked.connect(self.client.stop)
        frame.layout().addWidget(btn)

        frame.layout().addSpacing(20)
        frame.layout().addWidget(QLabel('Mode:'))
        frame.layout().addWidget(self.mode_combo)

        frame.layout().addSpacing(20)
        frame.layout().addStretch()

        replay_btn = icon_button('replay', 'Replay')
        replay_btn.clicked.connect(self.show_replay_window)
        frame.layout().addWidget(replay_btn)

        frame.layout().addSpacing(20)

        self.log_toggle = icon_button('show_log', 'Show Log')
        self.log_toggle.setCheckable(True)
        self.log_toggle.setShortcut(QKeySequence('Ctrl+L'))
        frame.layout().addWidget(self.log_toggle)

        about_btn = icon_button('live_help', 'About', cls=QToolButton)
        about_btn.clicked.connect(self.show_about)
        frame.layout().addWidget(about_btn)

        btn = icon_button('quit', 'Quit')
        btn.setShortcut(QKeySequence('Ctrl+Q'))
        btn.clicked.connect(QApplication.instance().quit)
        frame.layout().addWidget(btn)

        self.buttons_frame = frame

    def show_about(self):
        dialog = QDialog(self)
        dialog.setWindowTitle('About UMAMI GUI')

        # the pixel grid plays the count-buildup animation; the wordmark
        # below it is its own separate SVG (icons/wordmark.svg), not part
        # of the grid we draw ourselves
        logo_width = 160
        wordmark_height = round(logo_width * WORDMARK_ASPECT)

        grid = LogoBuildupWidget(logo_width)
        wordmark = QLabel()
        wordmark.setPixmap(load_icon('wordmark').pixmap(logo_width, wordmark_height))
        wordmark.setAlignment(Qt.AlignmentFlag.AlignCenter)

        logo_layout = QVBoxLayout()
        logo_layout.setContentsMargins(0, 0, 0, 0)
        logo_layout.setSpacing(12)
        logo_layout.addWidget(grid, alignment=Qt.AlignmentFlag.AlignCenter)
        logo_layout.addWidget(wordmark)

        authors_html = '<br>'.join(html.escape(author) for author in AUTHORS)
        text = QLabel(
            f'{__version__}<br><br>'
            'Live histogram viewer and control panel for the UMAMI '
            'data-acquisition backend.<br><br>'
            f'<b>Authors</b><br>{authors_html}')
        text.setAlignment(Qt.AlignmentFlag.AlignCenter)
        text.setWordWrap(True)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok)
        buttons.accepted.connect(dialog.accept)

        layout = QVBoxLayout(dialog)
        layout.addLayout(logo_layout)
        layout.addWidget(text)
        layout.addWidget(buttons)
        dialog.exec()

    def on_start_clicked(self):
        self.client.start(self.run_id_field.text() or
                          time.strftime('%Y-%m-%d_%H-%M-%S'))

    def _quick_setup_window(self, attr, factory, on_applied=None):
        """Get (creating on first use) one of the optional quick-setup windows.

        `on_applied`, if given, is connected to the window's `applied` signal
        right after construction -- so a live Apply there also refreshes the
        main params table instead of leaving it stale until manually pulled.
        """
        window = getattr(self, attr)
        if window is None:
            window = factory()
            if on_applied is not None:
                window.applied.connect(on_applied)
            setattr(self, attr, window)
        return window

    def show_aux_histo_window(self):
        window = self._quick_setup_window(
            'aux_histo_window',
            lambda: AuxHistoWindow(self.client, self.shm_name, self.log_panel),
            on_applied=self.refresh_params)
        window.show()
        window.raise_()

    def show_mcpd_config_window(self):
        window = self._quick_setup_window(
            'mcpd_config_window', lambda: McpdConfigWindow(self.client),
            on_applied=self.refresh_params)
        window.show()
        window.raise_()

    def show_tof_config_window(self):
        window = self._quick_setup_window(
            'tof_config_window',
            lambda: TofConfigWindow(self.client, lambda: self.histo.nt),
            on_applied=self.refresh_params)
        window.show()
        window.raise_()

    def show_replay_window(self):
        window = self._quick_setup_window(
            'replay_window', lambda: ReplayWindow(self.client),
            on_applied=self._on_replay_applied)
        window.show()
        window.raise_()

    def _on_replay_applied(self):
        self.refresh_params()
        # avoid re-dumping already-captured data back to disk
        self.raw_dump_check.setChecked(False)

    # ---- display controls: scale, colormap, levels, cursor readout ----

    def _build_display_controls(self):
        frame = QFrame()
        frame.setLayout(QHBoxLayout())
        frame.layout().setContentsMargins(8, 0, 8, 5)
        frame.setSizePolicy(QSizePolicy.Policy.Preferred,
                            QSizePolicy.Policy.Fixed)

        frame.layout().addWidget(QLabel('Show t slice:'))
        self.t_spin = QSpinBox()
        self.t_spin.setRange(0, max(self.histo.nt - 1, 0))
        self.t_spin.valueChanged.connect(lambda _: self.update_buffer())
        frame.layout().addWidget(self.t_spin)

        frame.layout().addSpacing(20)

        self.log_scale_check = QCheckBox('Log scale')
        self.log_scale_check.setChecked(True)
        self.log_scale_check.toggled.connect(lambda _: self.update_buffer())
        frame.layout().addWidget(self.log_scale_check)

        frame.layout().addSpacing(20)
        frame.layout().addWidget(QLabel('Colormap:'))
        self.colormap_combo = QComboBox()
        self.colormap_combo.addItems(list(COLORMAPS))
        self.colormap_combo.currentTextChanged.connect(
            lambda name: self.img.setColorMap(pg.colormap.get(COLORMAPS[name])))
        frame.layout().addWidget(self.colormap_combo)

        frame.layout().addSpacing(20)
        self.auto_levels_check = QCheckBox('Auto levels')
        self.auto_levels_check.setChecked(True)
        frame.layout().addWidget(self.auto_levels_check)

        self.level_min_spin = QDoubleSpinBox()
        self.level_min_spin.setRange(0, 1e9)
        self.level_min_spin.setDecimals(0)
        self.level_min_spin.setEnabled(False)
        frame.layout().addWidget(self.level_min_spin)

        frame.layout().addWidget(QLabel('-'))

        self.level_max_spin = QDoubleSpinBox()
        self.level_max_spin.setRange(0, 1e9)
        self.level_max_spin.setDecimals(0)
        self.level_max_spin.setValue(100)
        self.level_max_spin.setEnabled(False)
        frame.layout().addWidget(self.level_max_spin)

        self.auto_levels_check.toggled.connect(self.on_auto_levels_toggled)
        self.level_min_spin.valueChanged.connect(lambda _: self.update_buffer())
        self.level_max_spin.valueChanged.connect(lambda _: self.update_buffer())

        frame.layout().addSpacing(20)
        frame.layout().addStretch()

        self.cursor_label = QLabel('')
        self.cursor_label.setMinimumWidth(200)
        frame.layout().addWidget(self.cursor_label)

        self.display_frame = frame

    def on_auto_levels_toggled(self, checked):
        self.level_min_spin.setEnabled(not checked)
        self.level_max_spin.setEnabled(not checked)
        self.update_buffer()

    def on_mouse_moved(self, scene_pos):
        if not self.plot.sceneBoundingRect().contains(scene_pos):
            self.cursor_label.setText('')
            return
        view_pos = self.plot.vb.mapSceneToView(scene_pos)
        # the image is drawn shifted by (-0.5, -0.5) (see _build_main_plot)
        x = int(np.floor(view_pos.x() + 0.5))
        y = int(np.floor(view_pos.y() + 0.5))
        if (self.last_buf is not None
            and 0 <= y < self.last_buf.shape[0]
            and 0 <= x < self.last_buf.shape[1]
        ):
            self.cursor_label.setText(
                f'x={x}  y={y}  counts={int(self.last_buf[y, x])}')
        else:
            self.cursor_label.setText('')


    # ---- raw dump / save histo controls ----

    def _build_dump_controls(self):
        frame = QFrame()
        frame.setLayout(QHBoxLayout())
        frame.layout().setContentsMargins(8, 0, 8, 5)
        frame.setSizePolicy(QSizePolicy.Policy.Preferred,
                            QSizePolicy.Policy.Fixed)

        self.raw_dump_check = QCheckBox('Raw dump to:')
        frame.layout().addWidget(self.raw_dump_check)
        self.raw_dump_path = QLineEdit()
        self.raw_dump_path.setPlaceholderText('/path/to/raw/dump/dir')
        frame.layout().addWidget(self.raw_dump_path)
        self.raw_dump_check.toggled.connect(self._on_raw_dump_toggled)
        self.raw_dump_path.editingFinished.connect(self.raw_dump_path_changed)

        browse_btn = QPushButton('...')
        browse_btn.setMaximumWidth(30)
        browse_btn.clicked.connect(self.browse_raw_dump_dir)
        frame.layout().addWidget(browse_btn)

        frame.layout().addSpacing(20)

        btn = QPushButton('Diffractogram')
        btn.clicked.connect(self.open_projection_window)
        frame.layout().addWidget(btn)

        btn = QPushButton('TOF Spectrum')
        btn.clicked.connect(self.open_t_projection_window)
        frame.layout().addWidget(btn)

        self.aux_histo_btn = icon_button('histo', 'Aux Histograms')
        self.aux_histo_btn.clicked.connect(self.show_aux_histo_window)
        self.aux_histo_btn.setVisible(False)
        frame.layout().addWidget(self.aux_histo_btn)

        frame.layout().addSpacing(20)

        save_btn = icon_button('save', 'Save Histogram')
        save_menu = QMenu(save_btn)
        save_menu.addAction('ASCII text...', self.save_histo_dialog)
        save_menu.addAction('Image...', self.save_plot_image_dialog)
        save_btn.setMenu(save_menu)
        frame.layout().addWidget(save_btn)

        self.dump_frame = frame

    def _on_raw_dump_toggled(self, checked):
        if checked and not self.raw_dump_path.text():
            path = QFileDialog.getExistingDirectory(
                self, 'Raw Dump Directory', self.raw_dump_path.text())
            if not path:
                self.raw_dump_check.blockSignals(True)
                self.raw_dump_check.setChecked(False)
                self.raw_dump_check.blockSignals(False)
                return
            self.raw_dump_path.setText(path)
        self.client.set_raw_dump(checked, self.raw_dump_path.text())

    def browse_raw_dump_dir(self):
        path = QFileDialog.getExistingDirectory(
            self, 'Raw Dump Directory', self.raw_dump_path.text())
        if path:
            self.raw_dump_path.setText(path)
            self.raw_dump_path_changed()

    def raw_dump_path_changed(self):
        if self.raw_dump_check.isChecked():
            self.client.set_raw_dump(True, self.raw_dump_path.text())

    def save_histo_dialog(self):
        dialog = QFileDialog(
            self, 'Save Histogram', '', 'Text files (*.txt);;All files (*)')
        dialog.setAcceptMode(QFileDialog.AcceptMode.AcceptSave)
        dialog.setOption(QFileDialog.Option.DontUseNativeDialog, True)
        all_check = None
        if self.histo.nt > 1:
            all_check = QCheckBox('Save all t-slices (default: current only)')
            layout = dialog.layout()
            layout.addWidget(all_check, layout.rowCount(), 0, 1, layout.columnCount())
        if dialog.exec() != QDialog.DialogCode.Accepted:
            return
        paths = dialog.selectedFiles()
        if not paths:
            return
        path = paths[0]
        if not Path(path).suffix:
            path += '.txt'
        if all_check is not None and all_check.isChecked():
            self.client.save_histo(path, self.histo.nt)
        else:
            t = self.t_spin.value()
            buf = self.histo.read_plane(t)
            np.savetxt(path, buf, fmt='%d', header=self._histo_export_header(t, buf))
            self.log_panel.info(f'Saved t={t} slice to {path}')

    def _histo_export_header(self, t, buf):
        roi = self._roi_bounds()
        roi_total = int(buf[roi[1]:roi[3], roi[0]:roi[2]].sum()) if roi else 0
        total_events, total_neutrons, lifetime_ns, tzero_count, monitor_counts = \
            self.histo.read_counters()
        lines = [
            'UMAMI histogram export',
            f'run: {self.histo.run_id}',
            f'mode: {self.current_mode or self.mode_combo.currentText()}',
            f't-slice: {t}',
            f'counts in slice: {int(buf.sum())}',
            f'counts in ROI: {roi_total}',
            f'total counts: {total_neutrons}',
            f'life time: {lifetime_ns / 1_000_000_000:.1f} s',
            f'total events: {total_events}',
            f'tzero: {tzero_count}',
            *(f'monitor {i}: {c}' for i, c in enumerate(monitor_counts)),
        ]
        return '\n'.join(lines)

    def save_plot_image_dialog(self):
        path, _ = QFileDialog.getSaveFileName(
            self, 'Save Plot Image', '', 'PNG files (*.png);;All files (*)')
        if not path:
            return
        if not Path(path).suffix:
            path += '.png'
        exporters.ImageExporter(self.plot).export(path)

    def save_config_same_file(self):
        self.client.save_config()

    def save_config_dialog(self):
        path, _ = QFileDialog.getSaveFileName(
            self, 'Save Config', '', 'Config files (*.conf);;All files (*)')
        if not path:
            return
        if not Path(path).suffix:
            path += '.conf'
        self.client.save_config(path)

    # ---- params panel ----

    def _build_params_panel(self):
        panel = QWidget()
        panel.setLayout(QVBoxLayout())
        panel.layout().setContentsMargins(5, 8, 0, 0)
        self.params_tree = ParamsTree(self.client)

        top_row = QHBoxLayout()
        refresh_btn = icon_button('refresh', 'Refresh Params')
        refresh_btn.clicked.connect(self.refresh_params)
        top_row.addWidget(refresh_btn)

        save_config_btn = icon_button('save', 'Save Config')
        save_config_menu = QMenu(save_config_btn)
        save_config_menu.addAction('Same config file', self.save_config_same_file)
        save_config_menu.addAction('Select new file...', self.save_config_dialog)
        save_config_btn.setMenu(save_config_menu)
        top_row.addWidget(save_config_btn)
        top_row.addStretch()
        panel.layout().addLayout(top_row)

        # quick-setup dialogs for specific input types, shown only when
        # relevant to the current config -- MCPD Setup is the first of these
        setup_row = QHBoxLayout()
        self.mcpd_config_btn = QPushButton('MCPD Setup')
        self.mcpd_config_btn.clicked.connect(self.show_mcpd_config_window)
        self.mcpd_config_btn.setVisible(False)
        setup_row.addWidget(self.mcpd_config_btn)
        self.tof_config_btn = QPushButton('TOF Setup')
        self.tof_config_btn.clicked.connect(self.show_tof_config_window)
        self.tof_config_btn.setVisible(False)
        setup_row.addWidget(self.tof_config_btn)
        setup_row.addStretch()
        panel.layout().addLayout(setup_row)

        panel.layout().addWidget(self.params_tree)

        self.params_panel = panel

    def _build_events_panel(self):
        self.events_panel = EventsPanel()
        self.events_panel.set_roi_btn.toggled.connect(self._on_set_roi_toggled)

    def _on_set_roi_toggled(self, checked):
        if not checked:
            self.roi_item.hide()
            return
        size = self.roi_item.size()
        if size.x() <= 0 or size.y() <= 0:
            # never configured -- start from a sensible default the user
            # can then resize into place
            nx, ny = self.histo.nx, self.histo.ny
            x0, x1 = nx // 4, nx - nx // 4
            y0, y1 = ny // 4, ny - ny // 4
            self.roi_item.setPos((x0 - 0.5, y0 - 0.5))
            self.roi_item.setSize((x1 - x0, y1 - y0))
        self.roi_item.show()

    def _on_roi_region_changed(self):
        if self._roi_snap_guard:
            return
        self.roi_rate_samples.clear()
        # snap both edges to the nearest bin boundary (same convention as
        # _roi_bounds()/on_mouse_moved()) so the drawn box always lines up
        # with whole bins instead of an arbitrary sub-pixel rectangle
        pos, size = self.roi_item.pos(), self.roi_item.size()
        x0 = np.floor(pos.x() + 0.5) - 0.5
        x1 = np.floor(pos.x() + size.x() + 0.5) - 0.5
        y0 = np.floor(pos.y() + 0.5) - 0.5
        y1 = np.floor(pos.y() + size.y() + 0.5) - 0.5
        w, h = max(0.0, x1 - x0), max(0.0, y1 - y0)
        if (x0, y0, w, h) == (pos.x(), pos.y(), size.x(), size.y()):
            return
        self._roi_snap_guard = True
        try:
            self.roi_item.setPos((x0, y0))
            self.roi_item.setSize((w, h))
        finally:
            self._roi_snap_guard = False

    def _roi_bounds(self):
        """Return the current ROI as `(x0, y0, x1, y1)` bin-index bounds.

        `None` while the ROI is hidden/unset, or degenerate (e.g. dragged
        down to zero size).
        """
        if not self.roi_item.isVisible():
            return None
        pos = self.roi_item.pos()
        size = self.roi_item.size()
        # same pixel-center convention as on_mouse_moved() -- pixel i spans
        # [i-0.5, i+0.5], so floor(v + 0.5) gives the nearest bin boundary
        x0 = max(0, min(self.histo.nx, int(np.floor(pos.x() + 0.5))))
        x1 = max(0, min(self.histo.nx, int(np.floor(pos.x() + size.x() + 0.5))))
        y0 = max(0, min(self.histo.ny, int(np.floor(pos.y() + 0.5))))
        y1 = max(0, min(self.histo.ny, int(np.floor(pos.y() + size.y() + 0.5))))
        if x1 <= x0 or y1 <= y0:
            return None
        return x0, y0, x1, y1

    def _build_right_tabs(self):
        tabs = QTabWidget()
        tabs.setTabPosition(QTabWidget.TabPosition.North)
        tabs.setStyleSheet('QTabWidget::pane { border: 0; }')
        tabs.addTab(self.events_panel, 'Statistics')
        tabs.addTab(self.params_panel, 'Parameters')
        tabs.setCurrentWidget(self.events_panel)
        self.right_tabs = tabs

    # ---- assembly ----

    def _assemble(self):
        self.log_panel.setVisible(False)  # minimized by default; toggled via "Show Log"
        self.log_toggle.toggled.connect(self.log_panel.setVisible)
        self.log_panel.error_logged.connect(lambda: self.log_toggle.setChecked(True))

        self.left_splitter = QSplitter(Qt.Orientation.Vertical)
        self.left_splitter.addWidget(self.graphics)
        self.left_splitter.addWidget(self.log_panel)
        self.left_splitter.setSizes([600, 200])

        self.main_splitter = QSplitter(Qt.Orientation.Horizontal)
        self.main_splitter.addWidget(self.left_splitter)
        self.main_splitter.addWidget(self.right_tabs)
        self.main_splitter.setSizes([800, 300])
        self.main_splitter.setSizePolicy(QSizePolicy.Policy.Expanding,
                                         QSizePolicy.Policy.Expanding)

        self.layout().addWidget(self.buttons_frame)
        self.layout().addWidget(self.dump_frame)
        self.layout().addWidget(self._hline())
        self.layout().addWidget(self.status_panel)
        self.layout().addWidget(self._hline())
        self.layout().addWidget(self.display_frame)
        self.layout().addWidget(self.main_splitter)

    @staticmethod
    def _hline():
        line = QFrame()
        line.setFrameShape(QFrame.Shape.HLine)
        line.setFrameShadow(QFrame.Shadow.Sunken)
        return line

    # ---- persisted UI state ----

    def _load_settings(self):
        geometry = self.settings.value('geometry')
        if geometry is not None:
            self.restoreGeometry(geometry)
        main_state = self.settings.value('main_splitter')
        if main_state is not None:
            self.main_splitter.restoreState(main_state)
        left_state = self.settings.value('left_splitter')
        if left_state is not None:
            self.left_splitter.restoreState(left_state)
        colormap = self.settings.value('colormap')
        if colormap in COLORMAPS:
            self.colormap_combo.setCurrentText(colormap)
        self.log_scale_check.setChecked(
            self.settings.value('log_scale', True, type=bool))
        self.auto_levels_check.setChecked(
            self.settings.value('auto_levels', True, type=bool))
        self.level_min_spin.setValue(
            self.settings.value('level_min', 0, type=float))
        self.level_max_spin.setValue(
            self.settings.value('level_max', 100, type=float))
        # only the path is restored, not whether dumping was enabled -- silently
        # resuming a raw dump to a possibly stale path on startup would surprise
        raw_dump_path = self.settings.value('raw_dump_path')
        if raw_dump_path is not None:
            self.raw_dump_path.setText(raw_dump_path)
        if self.settings.contains('roi_active'):
            self.roi_item.setPos((self.settings.value('roi_x', type=float),
                                  self.settings.value('roi_y', type=float)))
            self.roi_item.setSize((self.settings.value('roi_w', type=float),
                                   self.settings.value('roi_h', type=float)))
            # setChecked triggers _on_set_roi_toggled(), which shows/hides
            # the already-restored geometry rather than defaulting it
            self.events_panel.set_roi_btn.setChecked(
                self.settings.value('roi_active', type=bool))

    def _save_settings(self):
        self.settings.setValue('geometry', self.saveGeometry())
        self.settings.setValue('main_splitter', self.main_splitter.saveState())
        self.settings.setValue('left_splitter', self.left_splitter.saveState())
        self.settings.setValue('colormap', self.colormap_combo.currentText())
        self.settings.setValue('log_scale', self.log_scale_check.isChecked())
        self.settings.setValue('auto_levels', self.auto_levels_check.isChecked())
        self.settings.setValue('level_min', self.level_min_spin.value())
        self.settings.setValue('level_max', self.level_max_spin.value())
        self.settings.setValue('raw_dump_path', self.raw_dump_path.text())
        pos, size = self.roi_item.pos(), self.roi_item.size()
        self.settings.setValue('roi_x', pos.x())
        self.settings.setValue('roi_y', pos.y())
        self.settings.setValue('roi_w', size.x())
        self.settings.setValue('roi_h', size.y())
        self.settings.setValue('roi_active', self.roi_item.isVisible())
