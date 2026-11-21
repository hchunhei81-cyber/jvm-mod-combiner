/**
 * Compatibility entry point for existing BusAgent call sites.
 * New entity synchronization logic lives in EntitySync.
 */
final class MonsterSync {
    private MonsterSync() {
    }

    static void sample(Object minecraft) {
        EntitySync.sample(minecraft);
    }

    static void acknowledge(java.util.UUID uuid, long seq) {
        EntitySync.acknowledge(uuid, seq);
    }
}
