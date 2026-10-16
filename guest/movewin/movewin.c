// SPDX-License-Identifier: GPL-3.0-only
/*
 * movewin.c - reposition EP147's top-level window to (0,0).
 *
 * EP147 creates a 1280x800 window and places it at (-640,-400) - its center at
 * the screen origin. On the real XDJ-AZ the DSI panel is physically rotated
 * (apl_start: `xrandr --output DSI-1 --rotate left`) so this lands correctly.
 * QEMU's virtio-gpu can't do a modesetting rotation (it wedges the screen to
 * 320x200), so instead we give EP147 a native 1280x800 landscape screen and
 * just move its window to (0,0) so the full UI is on-screen.
 *
 * Runs as a tiny standalone X client (see az-movewin.service). Loops because
 * EP147 may re-assert its position.
 */
#include <X11/Xlib.h>
#include <string.h>
#include <unistd.h>
#include <stdio.h>

int main(void)
{
	Display *d = NULL;
	int i;

	for (i = 0; i < 180 && !d; i++) {
		d = XOpenDisplay(":0");
		if (!d)
			sleep(1);
	}
	if (!d) {
		fprintf(stderr, "movewin: cannot open display :0\n");
		return 1;
	}

	Window root = DefaultRootWindow(d);
	for (;;) {
		Window r, parent, *kids = NULL;
		unsigned int n = 0;

		if (XQueryTree(d, root, &r, &parent, &kids, &n)) {
			for (unsigned int k = 0; k < n; k++) {
				char *name = NULL;
				if (XFetchName(d, kids[k], &name) && name) {
					if (strcmp(name, "EP147") == 0) {
						XWindowAttributes a;
						if (XGetWindowAttributes(d, kids[k], &a) &&
						    (a.x != 0 || a.y != 0))
							XMoveWindow(d, kids[k], 0, 0);
					}
					XFree(name);
				}
			}
			if (kids)
				XFree(kids);
		}
		XFlush(d);
		usleep(500000);
	}
	return 0;
}
