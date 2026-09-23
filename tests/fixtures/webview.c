// A WebKitGTK window like the one Tauri apps embed: a text area, a field and
// a page taller than the window, for typing and scrolling tests. Each
// keystroke in the text area costs 15 ms, as a framework re-rendering a
// development build does, so input that outruns the page shows.
#include <webkit2/webkit2.h>

static const char *PAGE =
    "<!doctype html><title>Webview proof</title>"
    "<textarea id=a aria-label=First style='width:90%;height:160px'></textarea><br>"
    "<input id=b aria-label=Second style='width:90%;height:32px'>"
    "<div style='height:3000px'></div>"
    "<button>Bottom button</button>"
    "<script>document.getElementById('a').addEventListener('input', () => {"
    "  const start = performance.now(); while (performance.now() - start < 15);"
    "});</script>";

int main(int argc, char **argv) {
    gtk_init(&argc, &argv);
    GtkWidget *window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(window), "Webview proof");
    gtk_window_set_default_size(GTK_WINDOW(window), 900, 600);
    GtkWidget *view = webkit_web_view_new();
    gtk_container_add(GTK_CONTAINER(window), view);
    webkit_web_view_load_html(WEBKIT_WEB_VIEW(view), PAGE, NULL);
    g_signal_connect(window, "destroy", G_CALLBACK(gtk_main_quit), NULL);
    gtk_widget_show_all(window);
    gtk_main();
    return 0;
}
