import java.util.concurrent.atomic.AtomicReference;

final class MonsterCleanupCoordinator {
    private final AtomicReference<Object> pending = new AtomicReference<>();

    Object tryAcquire() {
        Object lease = new Object();
        return pending.compareAndSet(null, lease) ? lease : null;
    }

    void release(Object lease) {
        if (lease != null) {
            pending.compareAndSet(lease, null);
        }
    }

    boolean isEligible(long current, long requested, boolean connected, int condition) {
        if (current != requested) return false;
        if (condition == 0) return !connected;
        if (condition == 2) return connected;
        return false;
    }

    boolean canReleaseBarrier(boolean removed, int condition,
            long current, long requested, boolean connected,
            boolean remoteEmpty, boolean barrierPending) {
        return removed && condition == 2 && current == requested
                && connected && remoteEmpty && barrierPending;
    }
}
