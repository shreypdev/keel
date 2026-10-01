package dev.undra.android

import dev.undra.runtime.adapters.NetKind
import org.junit.Assert.assertEquals
import org.junit.Test

/** How a network's capabilities become what the core is told. */
class NetworkClassifierTest {
    private fun classify(
        internet: Boolean = true,
        validated: Boolean = true,
        wifi: Boolean = false,
        cellular: Boolean = false,
        ethernet: Boolean = false,
        requireValidated: Boolean = false,
    ) = NetworkClassifier.classify(internet, validated, wifi, cellular, ethernet, requireValidated)

    @Test
    fun the_kind_is_the_most_specific_link_wifi_then_cellular_then_wired() {
        assertEquals(NetworkStatus(true, NetKind.WIFI), classify(wifi = true))
        assertEquals(NetworkStatus(true, NetKind.CELLULAR), classify(cellular = true))
        assertEquals(NetworkStatus(true, NetKind.WIRED), classify(ethernet = true))
        assertEquals(NetworkStatus(true, NetKind.WIFI), classify(wifi = true, cellular = true, ethernet = true))
        assertEquals(NetworkStatus(true, NetKind.CELLULAR), classify(cellular = true, ethernet = true))
    }

    @Test
    fun a_network_with_an_unknown_link_is_online_with_an_unknown_kind() {
        assertEquals(NetworkStatus(true, NetKind.UNKNOWN), classify())
    }

    @Test
    fun a_network_that_does_not_provide_internet_is_offline_whatever_its_link() {
        assertEquals(NetworkStatus.OFFLINE, classify(internet = false, wifi = true))
        assertEquals(NetKind.NONE, NetworkStatus.OFFLINE.kind)
    }

    @Test
    fun an_unvalidated_network_is_online_unless_validation_is_required() {
        assertEquals(NetworkStatus(true, NetKind.WIFI), classify(validated = false, wifi = true))
        assertEquals(NetworkStatus.OFFLINE, classify(validated = false, wifi = true, requireValidated = true))
        assertEquals(NetworkStatus(true, NetKind.WIFI), classify(validated = true, wifi = true, requireValidated = true))
    }

    @Test
    fun no_capabilities_means_no_network() {
        assertEquals(NetworkStatus.OFFLINE, NetworkClassifier.classify(null, requireValidated = false))
    }
}
