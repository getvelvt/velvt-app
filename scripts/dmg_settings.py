import os

application = os.path.abspath(defines["app"])  # noqa: F821
background = os.path.abspath(defines["background"])  # noqa: F821
volume_icon = os.path.abspath(defines["volume_icon"])  # noqa: F821

format = "UDZO"
compression_level = 9
filesystem = "HFS+"
files = [(application, "Velvt.app")]
symlinks = {"Applications": "/Applications"}
icon = volume_icon
icon_locations = {
    "Velvt.app": (165, 225),
    "Applications": (505, 225),
}

show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
window_rect = ((120, 120), (660, 420))
default_view = "icon-view"
show_icon_preview = False
include_icon_view_settings = True
arrange_by = None
label_pos = "bottom"
text_size = 14
icon_size = 96
