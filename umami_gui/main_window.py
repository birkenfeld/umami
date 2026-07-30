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
from pyqtgraph.Qt import QtCore, QtGui, QtWidgets

from . import __version__
from .aux_histo import AuxHistoWindow
from .axis_items import RotatedAxisItem, ZoomViewBox
from .client import UmamiClient
from .icons import icon_button, load_icon
from .log_panel import LogPanel
from .logo_widget import LogoBuildupWidget
from .params_table import ParamsTable
from .shm import ShmHistogram
from .status_panel import StatusPanel

IMAGE_REFRESH_MS = 250
STATE_POLL_MS = 1000

# display name -> pyqtgraph colormap name; pyqtgraph has no plain 'grey'/'gray'
# map, so this points at CET-L1, its linear (grayscale) perceptual map
COLORMAPS = {
    'viridis': 'viridis',
    'inferno': 'inferno',
    'plasma': 'plasma',
    'magma': 'magma',
    'turbo': 'turbo',
    'grey': 'CET-L1',
}

# height/width of icons/wordmark.svg's viewBox (199.328 x 44.863891) -- used
# to size the About dialog's wordmark without distorting it
WORDMARK_ASPECT = 44.863891 / 199.328

# kept in sync by hand with Cargo.toml's [package.authors]
AUTHORS = [
    'Georg Brandl <g.brandl@fz-juelich.de>',
    'Alexander Zaft <a.zaft@fz-juelich.de>',
    'Enrico Faulhaber <enrico.faulhaber@frm2.tum.de>',
]


class MainWindow(QtWidgets.QWidget):
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
        self.setLayout(QtWidgets.QVBoxLayout())
        self.layout().setContentsMargins(0, 0, 0, 0)

        try:
            self.histo = ShmHistogram(shm_name)
        except RuntimeError as e:
            QtWidgets.QMessageBox.critical(None, 'Error', str(e))
            sys.exit(1)

        self.log_panel = LogPanel()
        self.client = UmamiClient(shm_name, self.log_panel)
        self.aux_histo_window = AuxHistoWindow(self.client, shm_name, self.log_panel)

        self.proj_window = None
        self.proj_curve = None
        self.t_proj_window = None
        self.t_proj_curve = None
        # raw time_bins of the active mode (nanoseconds, trailing overflow
        # sentinel included), or None to fall back to plain bin-index labeling
        self.t_bin_edges_ns = None
        self.current_mode = None
        self.instance_name = None
        self.prev = {}
        self.last_t = None
        self.last_buf = None
        self.was_connected = False
        self.settings = QtCore.QSettings()

        self._build_main_plot()
        self._build_projection_state()
        self._build_status_and_mode()
        self._build_buttons()
        self._build_display_controls()
        self._build_dump_controls()
        self._build_params_panel()
        self._assemble()
        self._load_settings()
        QtWidgets.QApplication.instance().aboutToQuit.connect(self._save_settings)

        self.image_timer = QtCore.QTimer()
        self.image_timer.timeout.connect(self.update_buffer)
        self.image_timer.start(IMAGE_REFRESH_MS)

        self.state_timer = QtCore.QTimer()
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

    # ---- main image plot ----

    def _build_main_plot(self):
        # a plain Qt title bar, not pyqtgraph's own PlotItem.setTitle() --
        # that renders as a single centered/HTML label with no built-in way
        # to independently left/right-align two pieces of text across its
        # full width (its QGraphicsTextItem sizes to content, not to the
        # label's own allotted width)
        title_bar = QtWidgets.QWidget()
        title_layout = QtWidgets.QHBoxLayout(title_bar)
        title_layout.setContentsMargins(8, 4, 8, 2)
        self.title_left_label = QtWidgets.QLabel('starting...')
        self.title_right_label = QtWidgets.QLabel('')
        title_layout.addWidget(self.title_left_label)
        title_layout.addStretch()
        title_layout.addWidget(self.title_right_label)

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

        self.plot_container = QtWidgets.QWidget()
        container_layout = QtWidgets.QVBoxLayout(self.plot_container)
        container_layout.setContentsMargins(0, 0, 0, 0)
        container_layout.setSpacing(0)
        container_layout.addWidget(title_bar)
        container_layout.addWidget(self.graphics)

    # ---- projection plots: separate windows, created lazily on request ----

    def _build_projection_state(self):
        self.proj_x_edges = np.arange(self.histo.nx + 1) - 0.5
        self.t_proj_x_edges = np.arange(self.histo.nt + 1) - 0.5

    def open_projection_window(self):
        if self.proj_window is None:
            self.proj_window = pg.PlotWidget(
                title='Diffractogram', viewBox=ZoomViewBox())
            self.proj_window.setWindowTitle('UMAMI diffractogram')
            self.proj_window.setLabel('bottom', 'x channel')
            self.proj_window.setLabel('left', 'counts')
            self.proj_window.resize(700, 400)
            self.proj_curve = self.proj_window.plot(stepMode='center', fillLevel=0,
                                                     brush=(0, 0, 255, 80))
        self.proj_window.show()
        self.proj_window.raise_()

    def open_t_projection_window(self):
        if self.t_proj_window is None:
            self.t_proj_window = pg.PlotWidget(
                title='TOF spectrum', viewBox=ZoomViewBox(),
                axisItems={'bottom': RotatedAxisItem(orientation='bottom')})
            self.t_proj_window.setWindowTitle('UMAMI TOF spectrum')
            self.t_proj_window.setLabel('bottom', 't bin')
            self.t_proj_window.setLabel('left', 'counts')
            self.t_proj_window.getAxis('bottom').setHeight(70)
            self.t_proj_window.resize(700, 400)
            self.t_proj_curve = self.t_proj_window.plot(stepMode='center', fillLevel=0,
                                                         brush=(0, 0, 255, 80))
            self.apply_t_axis_ticks()
        self.t_proj_window.show()
        self.t_proj_window.raise_()

    @staticmethod
    def t_axis_tick_labels(edges_ns):
        labels = []
        lo = 0
        n = len(edges_ns)
        for i, hi in enumerate(edges_ns):
            if i == n - 1:
                labels.append('overflow')
            else:
                labels.append(f'{lo / 1e6:.3f}-{hi / 1e6:.3f}ms')
                lo = hi
        return labels

    def apply_t_axis_ticks(self):
        if self.t_proj_window is None:
            return
        axis = self.t_proj_window.getAxis('bottom')
        if self.t_bin_edges_ns is not None:
            labels = self.t_axis_tick_labels(self.t_bin_edges_ns)
            axis.setTicks([list(enumerate(labels))])
        else:
            axis.setTicks(None)

    def update_t_axis_labels(self):
        info = (self.params_table.params or {}).get(f'{self.current_mode}.time_bins')
        value = info.get('value') if info else None
        self.t_bin_edges_ns = value if value and len(value) > 1 else None
        self.apply_t_axis_ticks()

    def refresh_params(self):
        self.params_table.refresh()
        self.update_t_axis_labels()
        if self.aux_histo_window.isVisible():
            self.aux_histo_window.refresh()

    def update_buffer(self):
        t = self.t_spin.value()
        if t != self.last_t:
            # switching slices jumps the total to an unrelated bin's count;
            # drop the old baseline so the rate isn't computed across that jump
            self.prev.clear()
            self.last_t = t
        run_id = self.histo.read_run_id()
        buf = self.histo.read_plane(t)
        # a copy, not the raw mmap view -- keeping a view alive across ticks
        # would block the mmap from ever closing (e.g. on reconnect)
        self.last_buf = buf.copy()
        if self.log_scale_check.isChecked():
            display = np.log10(buf.astype(float) + 0.1)
        else:
            display = buf.astype(float)
        if self.auto_levels_check.isChecked():
            self.img.setImage(display, autoLevels=True)
        else:
            lo, hi = self.level_min_spin.value(), self.level_max_spin.value()
            if self.log_scale_check.isChecked():
                lo, hi = np.log10(lo + 0.1), np.log10(hi + 0.1)
            self.img.setImage(display, autoLevels=False, levels=(lo, hi))
        if self.proj_curve is not None and self.proj_window.isVisible():
            self.proj_curve.setData(self.proj_x_edges, buf.sum(axis=0),
                                    stepMode='center')
        if self.t_proj_curve is not None and self.t_proj_window.isVisible():
            if self.t_bin_edges_ns is not None:
                n = len(self.t_bin_edges_ns)
                edges = np.arange(n + 1) - 0.5
            else:
                n = self.histo.nt
                edges = self.t_proj_x_edges
            self.t_proj_curve.setData(edges, self.histo.read_time_projection(n),
                                      stepMode='center')

        total = int(buf.sum())
        now = time.monotonic()
        counts_text = f'{total} total counts'
        if self.prev and total >= self.prev['total']:
            rate = (total - self.prev['total']) / (now - self.prev['time'])
            counts_text += f' ({rate:,.1f}/sec)'
        self.title_left_label.setText(f'Run {run_id}')
        self.title_right_label.setText(counts_text)
        self.prev['total'] = total
        self.prev['time'] = now

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
        self.histo.close()
        self.histo = new_histo
        self.proj_x_edges = np.arange(self.histo.nx + 1) - 0.5
        self.t_proj_x_edges = np.arange(self.histo.nt + 1) - 0.5
        self.prev.clear()
        self.plot.enableAutoRange('xy', True)
        self.t_spin.setRange(0, max(self.histo.nt - 1, 0))

    # ---- status/state polling (also serves as the connection heartbeat) ----

    def _build_status_and_mode(self):
        self.status_panel = StatusPanel()
        self.mode_combo = QtWidgets.QComboBox()
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
        frame = QtWidgets.QFrame()
        frame.setLayout(QtWidgets.QHBoxLayout())
        frame.layout().setContentsMargins(8, 5, 8, 5)
        frame.setSizePolicy(QtWidgets.QSizePolicy.Policy.Preferred,
                            QtWidgets.QSizePolicy.Policy.Fixed)

        btn = icon_button('reset', 'Reset')
        btn.clicked.connect(self.client.reset)
        frame.layout().addWidget(btn)

        frame.layout().addWidget(QtWidgets.QLabel('Run ID:'))

        self.run_id_field = QtWidgets.QLineEdit()
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
        frame.layout().addWidget(QtWidgets.QLabel('Mode:'))
        frame.layout().addWidget(self.mode_combo)

        frame.layout().addSpacing(20)
        aux_histo_btn = icon_button('histo', 'Aux Histograms')
        aux_histo_btn.clicked.connect(self.show_aux_histo_window)
        frame.layout().addWidget(aux_histo_btn)

        frame.layout().addStretch()

        self.log_toggle = icon_button('show_log', 'Show Log')
        self.log_toggle.setCheckable(True)
        self.log_toggle.setShortcut(QtGui.QKeySequence('Ctrl+L'))
        frame.layout().addWidget(self.log_toggle)

        about_btn = QtWidgets.QPushButton('About')
        about_btn.clicked.connect(self.show_about)
        frame.layout().addWidget(about_btn)

        btn = icon_button('quit', 'Quit')
        btn.setShortcut(QtGui.QKeySequence('Ctrl+Q'))
        btn.clicked.connect(QtWidgets.QApplication.instance().quit)
        frame.layout().addWidget(btn)

        self.buttons_frame = frame

    def show_about(self):
        dialog = QtWidgets.QDialog(self)
        dialog.setWindowTitle('About UMAMI GUI')

        # the pixel grid plays the count-buildup animation; the wordmark
        # below it is its own separate SVG (icons/wordmark.svg), not part
        # of the grid we draw ourselves
        logo_width = 160
        wordmark_height = round(logo_width * WORDMARK_ASPECT)

        grid = LogoBuildupWidget(logo_width)
        wordmark = QtWidgets.QLabel()
        wordmark.setPixmap(load_icon('wordmark').pixmap(logo_width, wordmark_height))
        wordmark.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        logo_layout = QtWidgets.QVBoxLayout()
        logo_layout.setContentsMargins(0, 0, 0, 0)
        logo_layout.setSpacing(12)
        logo_layout.addWidget(grid, alignment=QtCore.Qt.AlignmentFlag.AlignCenter)
        logo_layout.addWidget(wordmark)

        authors_html = '<br>'.join(html.escape(author) for author in AUTHORS)
        text = QtWidgets.QLabel(
            f'{__version__}<br><br>'
            'Live histogram viewer and control panel for the UMAMI '
            'data-acquisition backend.<br><br>'
            f'<b>Authors</b><br>{authors_html}')
        text.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        text.setWordWrap(True)

        buttons = QtWidgets.QDialogButtonBox(
            QtWidgets.QDialogButtonBox.StandardButton.Ok)
        buttons.accepted.connect(dialog.accept)

        layout = QtWidgets.QVBoxLayout(dialog)
        layout.addLayout(logo_layout)
        layout.addWidget(text)
        layout.addWidget(buttons)
        dialog.exec()

    def on_start_clicked(self):
        self.client.start(self.run_id_field.text() or
                          time.strftime('%Y-%m-%d_%H:%M:%S'))

    def show_aux_histo_window(self):
        self.aux_histo_window.show()
        self.aux_histo_window.raise_()

    # ---- display controls: scale, colormap, levels, cursor readout ----

    def _build_display_controls(self):
        frame = QtWidgets.QFrame()
        frame.setLayout(QtWidgets.QHBoxLayout())
        frame.layout().setContentsMargins(8, 0, 8, 5)
        frame.setSizePolicy(QtWidgets.QSizePolicy.Policy.Preferred,
                            QtWidgets.QSizePolicy.Policy.Fixed)

        frame.layout().addWidget(QtWidgets.QLabel('Show t slice:'))
        self.t_spin = QtWidgets.QSpinBox()
        self.t_spin.setRange(0, max(self.histo.nt - 1, 0))
        self.t_spin.valueChanged.connect(lambda _: self.update_buffer())
        frame.layout().addWidget(self.t_spin)

        frame.layout().addSpacing(20)

        self.log_scale_check = QtWidgets.QCheckBox('Log scale')
        self.log_scale_check.setChecked(True)
        self.log_scale_check.toggled.connect(lambda _: self.update_buffer())
        frame.layout().addWidget(self.log_scale_check)

        frame.layout().addSpacing(20)
        frame.layout().addWidget(QtWidgets.QLabel('Colormap:'))
        self.colormap_combo = QtWidgets.QComboBox()
        self.colormap_combo.addItems(list(COLORMAPS))
        self.colormap_combo.currentTextChanged.connect(
            lambda name: self.img.setColorMap(pg.colormap.get(COLORMAPS[name])))
        frame.layout().addWidget(self.colormap_combo)

        frame.layout().addSpacing(20)
        self.auto_levels_check = QtWidgets.QCheckBox('Auto levels')
        self.auto_levels_check.setChecked(True)
        frame.layout().addWidget(self.auto_levels_check)

        self.level_min_spin = QtWidgets.QDoubleSpinBox()
        self.level_min_spin.setRange(0, 1e9)
        self.level_min_spin.setDecimals(0)
        self.level_min_spin.setEnabled(False)
        frame.layout().addWidget(self.level_min_spin)

        frame.layout().addWidget(QtWidgets.QLabel('-'))

        self.level_max_spin = QtWidgets.QDoubleSpinBox()
        self.level_max_spin.setRange(0, 1e9)
        self.level_max_spin.setDecimals(0)
        self.level_max_spin.setValue(100)
        self.level_max_spin.setEnabled(False)
        frame.layout().addWidget(self.level_max_spin)

        self.auto_levels_check.toggled.connect(self.on_auto_levels_toggled)
        self.level_min_spin.valueChanged.connect(lambda _: self.update_buffer())
        self.level_max_spin.valueChanged.connect(lambda _: self.update_buffer())

        frame.layout().addStretch()

        self.cursor_label = QtWidgets.QLabel('')
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
        x, y = int(np.floor(view_pos.x())), int(np.floor(view_pos.y()))
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
        frame = QtWidgets.QFrame()
        frame.setLayout(QtWidgets.QHBoxLayout())
        frame.layout().setContentsMargins(8, 0, 8, 5)
        frame.setSizePolicy(QtWidgets.QSizePolicy.Policy.Preferred,
                            QtWidgets.QSizePolicy.Policy.Fixed)

        self.raw_dump_check = QtWidgets.QCheckBox('Raw dump to:')
        frame.layout().addWidget(self.raw_dump_check)
        self.raw_dump_path = QtWidgets.QLineEdit()
        self.raw_dump_path.setPlaceholderText('/path/to/raw/dump/dir')
        frame.layout().addWidget(self.raw_dump_path)
        self.raw_dump_check.toggled.connect(
            lambda checked: self.client.set_raw_dump(checked,
                                                     self.raw_dump_path.text()))

        browse_btn = QtWidgets.QPushButton('...')
        browse_btn.setMaximumWidth(30)
        browse_btn.clicked.connect(self.browse_raw_dump_dir)
        frame.layout().addWidget(browse_btn)

        frame.layout().addSpacing(20)

        btn = QtWidgets.QPushButton('Diffractogram')
        btn.clicked.connect(self.open_projection_window)
        frame.layout().addWidget(btn)

        btn = QtWidgets.QPushButton('TOF Spectrum')
        btn.clicked.connect(self.open_t_projection_window)
        frame.layout().addWidget(btn)

        frame.layout().addSpacing(20)

        btn = icon_button('save', 'Save Histogram...')
        btn.clicked.connect(self.save_histo_dialog)
        frame.layout().addWidget(btn)

        btn = icon_button('save', 'Save Plot PNG...')
        btn.clicked.connect(self.save_plot_image_dialog)
        frame.layout().addWidget(btn)

        self.dump_frame = frame

    def browse_raw_dump_dir(self):
        path = QtWidgets.QFileDialog.getExistingDirectory(
            self, 'Raw Dump Directory', self.raw_dump_path.text())
        if path:
            self.raw_dump_path.setText(path)

    def save_histo_dialog(self):
        dialog = QtWidgets.QFileDialog(
            self, 'Save Histogram', '', 'Text files (*.txt);;All files (*)')
        dialog.setAcceptMode(QtWidgets.QFileDialog.AcceptMode.AcceptSave)
        dialog.setOption(QtWidgets.QFileDialog.Option.DontUseNativeDialog, True)
        all_check = None
        if self.histo.nt > 1:
            all_check = QtWidgets.QCheckBox('Save all t-slices (default: current only)')
            layout = dialog.layout()
            layout.addWidget(all_check, layout.rowCount(), 0, 1, layout.columnCount())
        if dialog.exec() != QtWidgets.QDialog.DialogCode.Accepted:
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
            np.savetxt(path, self.histo.read_plane(t), fmt='%d')
            self.log_panel.info(f'Saved t={t} slice to {path}')

    def save_plot_image_dialog(self):
        path, _ = QtWidgets.QFileDialog.getSaveFileName(
            self, 'Save Plot Image', '', 'PNG files (*.png);;All files (*)')
        if not path:
            return
        if not Path(path).suffix:
            path += '.png'
        exporters.ImageExporter(self.plot).export(path)

    # ---- params panel ----

    def _build_params_panel(self):
        panel = QtWidgets.QWidget()
        panel.setLayout(QtWidgets.QVBoxLayout())
        panel.layout().setContentsMargins(5, 0, 8, 5)
        self.params_table = ParamsTable(self.client)
        refresh_btn = icon_button('refresh', 'Refresh Params')
        refresh_btn.clicked.connect(self.refresh_params)
        panel.layout().addWidget(refresh_btn)
        panel.layout().addWidget(self.params_table)
        self.params_panel = panel

    # ---- assembly ----

    def _assemble(self):
        self.log_panel.setVisible(False)  # minimized by default; toggled via "Show Log"
        self.log_toggle.toggled.connect(self.log_panel.setVisible)
        self.log_panel.error_logged.connect(lambda: self.log_toggle.setChecked(True))

        self.left_splitter = QtWidgets.QSplitter(QtCore.Qt.Orientation.Vertical)
        self.left_splitter.addWidget(self.plot_container)
        self.left_splitter.addWidget(self.log_panel)
        self.left_splitter.setSizes([600, 200])

        self.main_splitter = QtWidgets.QSplitter(QtCore.Qt.Orientation.Horizontal)
        self.main_splitter.addWidget(self.left_splitter)
        self.main_splitter.addWidget(self.params_panel)
        self.main_splitter.setSizes([800, 300])
        self.main_splitter.setSizePolicy(QtWidgets.QSizePolicy.Policy.Expanding,
                                         QtWidgets.QSizePolicy.Policy.Expanding)

        self.layout().addWidget(self.buttons_frame)
        self.layout().addWidget(self.dump_frame)
        self.layout().addWidget(self.display_frame)
        self.layout().addWidget(self.status_panel)
        self.layout().addWidget(self.main_splitter)

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
        self.log_toggle.setChecked(
            self.settings.value('log_visible', False, type=bool))
        colormap = self.settings.value('colormap')
        if colormap in COLORMAPS:
            self.colormap_combo.setCurrentText(colormap)
        self.log_scale_check.setChecked(
            self.settings.value('log_scale', True, type=bool))
        self.auto_levels_check.setChecked(
            self.settings.value('auto_levels', True, type=bool))
        # only the path is restored, not whether dumping was enabled -- silently
        # resuming a raw dump to a possibly stale path on startup would surprise
        raw_dump_path = self.settings.value('raw_dump_path')
        if raw_dump_path is not None:
            self.raw_dump_path.setText(raw_dump_path)

    def _save_settings(self):
        self.settings.setValue('geometry', self.saveGeometry())
        self.settings.setValue('main_splitter', self.main_splitter.saveState())
        self.settings.setValue('left_splitter', self.left_splitter.saveState())
        self.settings.setValue('log_visible', self.log_toggle.isChecked())
        self.settings.setValue('colormap', self.colormap_combo.currentText())
        self.settings.setValue('log_scale', self.log_scale_check.isChecked())
        self.settings.setValue('auto_levels', self.auto_levels_check.isChecked())
        self.settings.setValue('raw_dump_path', self.raw_dump_path.text())
