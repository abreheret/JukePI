package app.musicremote

import java.text.Normalizer

/** Browse and search what is on the phone, for when the server is out of reach. Accents and case are ignored, like the server's search. */
object LocalCatalog {
    private val diacritics = Regex("\\p{Mn}+")
    private val nonWord = Regex("[^\\p{L}\\p{N}]+")

    fun norm(s: String): String = Normalizer.normalize(s.lowercase(), Normalizer.Form.NFD).replace(diacritics, "")

    private fun present() = LocalLibrary.albums.value.filter { it.tracksOnPhone > 0 }

    fun albums(artist: String? = null): List<Album> =
        present().map { it.detail.album }.filter { artist == null || it.artist.equals(artist, ignoreCase = true) }

    fun artists(): List<ArtistItem> =
        albums().filter { !it.variousArtists && !it.artist.isNullOrBlank() }
            .groupBy { it.artist!! }.map { (name, list) -> ArtistItem(name, list.size) }
            .sortedBy { norm(it.name) }

    fun search(query: String): SearchResult {
        val tokens = norm(query).split(nonWord).filter { it.isNotEmpty() }
        if (tokens.isEmpty()) return SearchResult(query)
        fun matches(vararg fields: String?): Boolean {
            val hay = norm(fields.filterNotNull().joinToString(" "))
            return tokens.all { hay.contains(it) }
        }
        val albums = present().map { it.detail }
        return SearchResult(
            query = query,
            albums = albums.map { it.album }.filter { matches(it.name, it.artist) }.take(12),
            artists = artists().filter { matches(it.name) }.take(12),
            tracks = albums.flatMap { d -> d.tracks.filter { matches(it.title, it.artist ?: d.album.artist, d.album.name) }.map { it.copy(albumName = d.album.name) } }
                .filter { LocalLibrary.trackFile(it.id) != null }.take(30),
        )
    }
}
