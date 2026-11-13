package app.musicremote

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
data class Album(
    val id: Long,
    val name: String,
    val artist: String? = null,
    @SerialName("various_artists") val variousArtists: Boolean = false,
    val year: Int? = null,
    @SerialName("track_count") val trackCount: Int = 0,
    @SerialName("has_cover") val hasCover: Boolean = false,
) {
    val artistLabel: String get() = if (variousArtists) "Artistes divers" else artist.orEmpty()
}

@Serializable
data class AlbumsPage(val total: Int = 0, val items: List<Album> = emptyList())

@Serializable
data class Track(
    val id: Long,
    @SerialName("album_id") val albumId: Long,
    val title: String,
    val artist: String? = null,
    @SerialName("track_no") val trackNo: Int? = null,
    @SerialName("duration_ms") val durationMs: Long? = null,
    @SerialName("album_name") val albumName: String? = null,
)

@Serializable
data class AlbumDetail(val album: Album, val tracks: List<Track> = emptyList())

@Serializable
data class ArtistItem(val name: String, @SerialName("album_count") val albumCount: Int = 0)

@Serializable
data class ArtistsPage(val total: Int = 0, val items: List<ArtistItem> = emptyList())

@Serializable
data class SearchResult(
    val query: String = "",
    val albums: List<Album> = emptyList(),
    val artists: List<ArtistItem> = emptyList(),
    val tracks: List<Track> = emptyList(),
)

@Serializable
data class QueueItem(
    @SerialName("track_id") val trackId: Long,
    @SerialName("album_id") val albumId: Long,
    val title: String,
    val artist: String? = null,
    @SerialName("duration_ms") val durationMs: Long? = null,
)

/** Mirror of the server's GET /player snapshot. `status` is stopped | loading | playing | paused. */
@Serializable
data class PlayerSnapshot(
    val status: String = "stopped",
    @SerialName("heos_connected") val heosConnected: Boolean = false,
    val index: Int? = null,
    val current: QueueItem? = null,
    @SerialName("position_ms") val positionMs: Long = 0,
    @SerialName("duration_ms") val durationMs: Long = 0,
    val volume: Int? = null,
    val mute: Boolean = false,
    val queue: List<QueueItem> = emptyList(),
    val error: String? = null,
)

/** Latest snapshot plus the local time it arrived: position is extrapolated from that moment, never from the Pi's clock. */
data class PlayerUi(val snap: PlayerSnapshot = PlayerSnapshot(), val receivedAt: Long = 0, val live: Boolean = false)
