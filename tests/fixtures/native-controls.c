/* Real GTK widgets exercise accessible values, constrained windows and XEmbed. */
#include <gtk/gtk.h>
static GtkWidget *primary, *status;
static GtkStatusIcon *tray;
static void activated(GtkWidget *unused, gpointer data) {
    (void)unused; (void)data;
    gtk_label_set_text(GTK_LABEL(status), "Tray action activated");
    gtk_window_present(GTK_WINDOW(primary));
}
static void popup(GtkStatusIcon *icon, guint button, guint time, gpointer data) {
    (void)data;
    GtkWidget *menu = gtk_menu_new();
    GtkWidget *item = gtk_menu_item_new_with_label("Mark tray action");
    g_signal_connect(item, "activate", G_CALLBACK(activated), NULL);
    gtk_menu_shell_append(GTK_MENU_SHELL(menu), item);
    gtk_widget_show_all(menu);
    gtk_menu_popup(GTK_MENU(menu), NULL, NULL, gtk_status_icon_position_menu, icon, button, time);
}
static gboolean close_requested(GtkWidget *window, GdkEvent *event, gpointer data) {
    (void)event; (void)data;
    GtkWidget *dialog = gtk_message_dialog_new(GTK_WINDOW(window), GTK_DIALOG_MODAL,
        GTK_MESSAGE_QUESTION, GTK_BUTTONS_NONE, "Confirm closing the test app");
    gtk_dialog_add_buttons(GTK_DIALOG(dialog), "Keep open", GTK_RESPONSE_CANCEL,
        "Close app", GTK_RESPONSE_ACCEPT, NULL);
    gint response = gtk_dialog_run(GTK_DIALOG(dialog));
    gtk_widget_destroy(dialog);
    if (response == GTK_RESPONSE_ACCEPT) gtk_main_quit();
    return TRUE;
}
int main(int argc, char **argv) {
    gtk_init(&argc, &argv);
    primary = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(primary), "Native acceptance");
    gtk_window_set_default_size(GTK_WINDOW(primary), 850, 650);
    gtk_widget_set_size_request(primary, 640, 400);
    GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 18);
    gtk_container_set_border_width(GTK_CONTAINER(box), 24);
    gtk_container_add(GTK_CONTAINER(primary), box);
    status = gtk_label_new("Primary acceptance window");
    gtk_box_pack_start(GTK_BOX(box), status, FALSE, FALSE, 0);
    GtkWidget *entry = gtk_entry_new();
    gtk_entry_set_text(GTK_ENTRY(entry), "Editable native value");
    atk_object_set_name(gtk_widget_get_accessible(entry), "Native clipboard field");
    gtk_box_pack_start(GTK_BOX(box), entry, FALSE, FALSE, 0);
    GtkWidget *check = gtk_check_button_new_with_label("Native checked state");
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(check), TRUE);
    gtk_box_pack_start(GTK_BOX(box), check, FALSE, FALSE, 0);
    GtkWidget *password = gtk_entry_new();
    gtk_entry_set_visibility(GTK_ENTRY(password), FALSE);
    gtk_entry_set_text(GTK_ENTRY(password), "hidden-native-password");
    gtk_box_pack_start(GTK_BOX(box), password, FALSE, FALSE, 0);
    g_signal_connect(primary, "delete-event", G_CALLBACK(close_requested), NULL);
    GtkWidget *second = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(second), "Native acceptance");
    gtk_window_set_default_size(GTK_WINDOW(second), 420, 230);
    gtk_container_add(GTK_CONTAINER(second), gtk_label_new("Secondary acceptance window"));
    gtk_widget_show_all(primary); gtk_widget_show_all(second);
    gtk_window_move(GTK_WINDOW(primary), 30, 80);
    gtk_window_move(GTK_WINDOW(second), 1000, 120);
    tray = gtk_status_icon_new_from_icon_name("dialog-information");
    gtk_status_icon_set_tooltip_text(tray, "Toad tray acceptance");
    g_signal_connect(tray, "activate", G_CALLBACK(activated), NULL);
    g_signal_connect(tray, "popup-menu", G_CALLBACK(popup), NULL);
    gtk_main();
    g_object_unref(tray);
    return 0;
}
