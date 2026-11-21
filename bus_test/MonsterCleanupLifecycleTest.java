import java.util.ArrayDeque;
import java.util.Queue;

public final class MonsterCleanupLifecycleTest {
    interface Cleanup {
        boolean remove();
    }

    static final class Lifecycle {
        private long generation;
        private boolean connected;
        private boolean barrier;
        private final MonsterCleanupCoordinator coordinator =
                new MonsterCleanupCoordinator();
        private final Queue<Runnable> mainThread = new ArrayDeque<>();
        private boolean rejectNextSchedule;
        private Cleanup cleanup = () -> true;
        private int sequenceClears;

        synchronized long connect() {
            generation++;
            connected = true;
            barrier = true;
            return generation;
        }

        synchronized long disconnect() {
            generation++;
            connected = false;
            barrier = true;
            return generation;
        }

        synchronized boolean blocked() {
            return barrier;
        }

        synchronized int sequenceClears() {
            return sequenceClears;
        }

        synchronized void setCleanup(Cleanup value) {
            cleanup = value;
        }

        synchronized void rejectNextSchedule() {
            rejectNextSchedule = true;
        }

        void schedule(long requestedGeneration, boolean connectedCondition) {
            final Object lease = coordinator.tryAcquire();
            if (lease == null) {
                return;
            }

            synchronized (this) {
                if (rejectNextSchedule) {
                    rejectNextSchedule = false;
                    coordinator.release(lease);
                    return;
                }
                mainThread.add(() -> execute(
                        requestedGeneration, connectedCondition, lease));
            }
        }

        private void execute(long requestedGeneration,
                             boolean connectedCondition,
                             Object lease) {
            try {
                synchronized (this) {
                    if (!coordinator.isEligible(
                            generation,
                            requestedGeneration,
                            connected,
                            connectedCondition ? 2 : 0)) {
                        return;
                    }
                }

                boolean removed = cleanup.remove();
                synchronized (this) {
                    if (coordinator.canReleaseBarrier(
                            removed,
                            connectedCondition ? 2 : 0,
                            generation,
                            requestedGeneration,
                            connected,
                            true,
                            barrier)) {
                        sequenceClears++;
                        barrier = false;
                    }
                }
            } finally {
                coordinator.release(lease);
            }
        }

        void monitor() {
            final long current;
            final boolean currentConnected;
            synchronized (this) {
                current = generation;
                currentConnected = connected;
                if (currentConnected && !barrier) {
                    return;
                }
            }
            schedule(current, currentConnected);
        }

        void runNext() {
            Runnable task;
            synchronized (this) {
                task = mainThread.poll();
            }
            if (task == null) {
                throw new AssertionError("Expected queued main-thread task");
            }
            task.run();
        }

        synchronized int queuedTasks() {
            return mainThread.size();
        }
    }

    private static void check(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
        System.out.println("[PASS] " + message);
    }

    private static void delayedOldGeneration() {
        Lifecycle state = new Lifecycle();

        long oldGeneration = state.disconnect();
        state.schedule(oldGeneration, false);

        long newGeneration = state.connect();
        state.schedule(newGeneration, true);

        check(state.queuedTasks() == 1,
                "old pending task prevents duplicate scheduling");

        state.runNext();

        check(state.blocked(),
                "stale task cannot clear reconnect barrier");

        state.monitor();
        check(state.queuedTasks() == 1,
                "monitor retries after stale task releases pending flag");

        state.runNext();
        check(!state.blocked(),
                "new generation cleanup eventually clears barrier");

        check(state.sequenceClears() == 1,
                "sequence state cleared exactly once");
    }

    private static void schedulingFailureRetries() {
        Lifecycle state = new Lifecycle();
        long generation = state.connect();

        state.rejectNextSchedule();
        state.schedule(generation, true);

        check(state.queuedTasks() == 0,
                "rejected scheduling does not enqueue a task");

        state.monitor();
        check(state.queuedTasks() == 1,
                "monitor retries after scheduling failure");

        state.runNext();
        check(!state.blocked(),
                "successful retry releases reconnect barrier");
    }

    private static void cleanupFailureKeepsBarrier() {
        Lifecycle state = new Lifecycle();
        long generation = state.connect();

        state.setCleanup(() -> false);
        state.schedule(generation, true);
        state.runNext();

        check(state.blocked(),
                "failed entity cleanup keeps reconnect barrier");

        check(state.sequenceClears() == 0,
                "failed cleanup does not clear sequence state");

        state.setCleanup(() -> true);
        state.monitor();
        state.runNext();

        check(!state.blocked(),
                "later successful cleanup releases barrier");

        check(state.sequenceClears() == 1,
                "sequence state clears only after successful cleanup");
    }

    private static void staleTaskCannotClearNewBarrier() {
        Lifecycle state = new Lifecycle();

        long first = state.connect();
        state.schedule(first, true);

        long second = state.disconnect();
        state.connect();

        state.runNext();

        check(state.blocked(),
                "old connected-generation task cannot release newer barrier");

        check(state.sequenceClears() == 0,
                "stale task cannot clear sequence state");
    }

    private static void leaseOwnership() {
        MonsterCleanupCoordinator coordinator =
                new MonsterCleanupCoordinator();

        Object first = coordinator.tryAcquire();
        check(first != null, "first cleanup lease acquired");

        coordinator.release(first);
        Object second = coordinator.tryAcquire();
        check(second != null, "second cleanup lease acquired");

        coordinator.release(first);
        check(coordinator.tryAcquire() == null,
                "stale lease cannot release a newer cleanup task");

        coordinator.release(second);
    }

    private static void barrierRequiresEmptyRemoteMap() {
        MonsterCleanupCoordinator coordinator =
                new MonsterCleanupCoordinator();

        check(!coordinator.canReleaseBarrier(
                        true, 2, 7L, 7L, true, false, true),
                "non-empty remote map keeps reconnect barrier closed");
    }

    public static void main(String[] args) {
        leaseOwnership();
        barrierRequiresEmptyRemoteMap();
        delayedOldGeneration();
        schedulingFailureRetries();
        cleanupFailureKeepsBarrier();
        staleTaskCannotClearNewBarrier();
        System.out.println("[PASS] MonsterCleanupLifecycleTest: 18 assertions");
    }
}
