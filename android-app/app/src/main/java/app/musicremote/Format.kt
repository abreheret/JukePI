package app.musicremote

fun formatMs(ms: Long?): String {
    if (ms == null || ms <= 0) return "0:00"
    val s = ms / 1000
    return "%d:%02d".format(s / 60, s % 60)
}
