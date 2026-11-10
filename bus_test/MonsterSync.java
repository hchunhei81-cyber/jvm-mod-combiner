import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.HashMap;
import java.util.Map;
import java.util.UUID;

/**
 * Fabric-side hostile mob discovery and sampling.
 *
 * M1 scope:
 * - Discover vanilla Monster instances in the current ClientLevel.
 * - Track them by UUID.
 * - Sample position, rotation, type and health.
 * - Log discovery and periodic summaries only.
 *
 * This class does not send network messages or create remote entities.
 * All calls must be made on the Minecraft client main thread.
 */
final class MonsterSync {

    private static final long SUMMARY_INTERVAL_MS = 1000L;

    private static final long SAMPLE_INTERVAL_NS = 200_000_000L;
    private static long lastSampleAtNanos;

    private static Field levelField;
    // ClientLevel identity observed by the sampling thread.
    private static Object lastLevel;

    private static Field entityStorageField;
    private static Method managerGetGetterMethod;
    private static Method entityGetterGetAllMethod;
    private static Method entityGetterGetByUuidMethod;

    private static Method getTypeMethod;
    private static Method getRegistryKeyMethod;
    private static Object entityTypeRegistry;
    private static Field uuidField;
    private static Method getXMethod;
    private static Method getYMethod;
    private static Method getZMethod;
    private static Method getYawMethod;
    private static Method getPitchMethod;
    private static Method isAliveMethod;
    private static Method isRemovedMethod;
    private static Method getHealthMethod;
    private static Method getMaxHealthMethod;

    private static Class<?> monsterClass;
    private static boolean initialized;
    private static boolean errorPrinted;

    private static long lastSummaryAt;

    private static final Object STATE_LOCK = new Object();

    // Only states confirmed by NeoForge belong in previous.
    private static final Map<UUID, MobState> previous =
        new HashMap<>();

    // One latest unconfirmed state per monster UUID.
    private static final Map<UUID, PendingMonster> pending =
        new HashMap<>();

    private static final java.util.concurrent.atomic.AtomicLong NEXT_SEQUENCE =
        new java.util.concurrent.atomic.AtomicLong(0L);

    private MonsterSync() {
    }

    private static long nextSequence() {
        long value = NEXT_SEQUENCE.incrementAndGet();

        if (value <= 0L) {
            throw new IllegalStateException(
                "Monster sequence exhausted"
            );
        }

        return value;
    }

    private static long retryDelayNanos(int attempts) {
        if (attempts <= 1) {
            return 1_000_000_000L;
        }

        if (attempts == 2) {
            return 2_000_000_000L;
        }

        return 5_000_000_000L;
    }

    // Called by the DataBus receive thread.
    static void acknowledge(UUID uuid, long seq) {
        if (uuid == null || seq <= 0L) {
            return;
        }

        MobState confirmed = null;

        synchronized (STATE_LOCK) {
            PendingMonster item = pending.get(uuid);

            if (item == null || item.seq != seq) {
                System.out.println(
                    "[MonsterSync][ACK][STALE] uuid=" +
                    uuid + " seq=" + seq
                );
                return;
            }

            confirmed = item.state;
            previous.put(uuid, confirmed);
            pending.remove(uuid);
        }

        System.out.println(
            "[MonsterSync][ACK] type=" + confirmed.type +
            " uuid=" + uuid +
            " seq=" + seq +
            " pos=" + confirmed.position()
        );
    }

    private static final class PendingMonster {
        final MobState state;
        final long seq;
        final String messageType;

        long lastSentAtNanos;
        int attempts;

        PendingMonster(
            MobState state,
            long seq,
            String messageType,
            long nowNanos
        ) {
            this.state = state;
            this.seq = seq;
            this.messageType = messageType;
            this.lastSentAtNanos = nowNanos;
            this.attempts = 1;
        }
    }

    static void sample(Object minecraft) {
        if (minecraft == null) {
            return;
        }

        long sampleNow = System.nanoTime();

        if (lastSampleAtNanos != 0L &&
            sampleNow - lastSampleAtNanos < SAMPLE_INTERVAL_NS) {
            return;
        }

        lastSampleAtNanos = sampleNow;

        try {
            initialize(minecraft);

            Object level = levelField.get(minecraft);

            if (level == null) {
                int clearedPrevious;
                int clearedPending;

                lastLevel = null;

                synchronized (STATE_LOCK) {
                    clearedPrevious = previous.size();
                    clearedPending = pending.size();
                    previous.clear();
                    pending.clear();
                }

                if (clearedPrevious != 0 || clearedPending != 0) {
                    System.out.println(
                        "[MonsterSync][WORLD] ClientLevel unavailable; " +
                        "confirmed=" + clearedPrevious +
                        " pending=" + clearedPending +
                        " cleared"
                    );
                }

                return;
            }

            // A dimension/world transition may replace ClientLevel
            // without an intermediate null level.
            if (lastLevel != null && lastLevel != level) {
                int clearedPrevious;
                int clearedPending;

                synchronized (STATE_LOCK) {
                    clearedPrevious = previous.size();
                    clearedPending = pending.size();
                    previous.clear();
                    pending.clear();
                }

                System.out.println(
                    "[MonsterSync][WORLD] ClientLevel changed; " +
                    "confirmed=" + clearedPrevious +
                    " pending=" + clearedPending +
                    " cleared"
                );
            }

            lastLevel = level;

            Object storage = entityStorageField.get(level);

            if (storage == null) {
                throw new IllegalStateException(
                    "ClientLevel entity storage is null"
                );
            }

            Object getter =
                managerGetGetterMethod.invoke(storage);

            if (getter == null) {
                throw new IllegalStateException(
                    "EntityGetter is null"
                );
            }

            Object result =
                entityGetterGetAllMethod.invoke(getter);

            if (!(result instanceof Iterable<?>)) {
                throw new IllegalStateException(
                    "EntityGetter.getAll() did not return Iterable"
                );
            }

            Map<UUID, MobState> current = new HashMap<>();

            int sampled = 0;
            int changed = 0;

            for (Object entity : (Iterable<?>) result) {
                if (entity == null || !monsterClass.isInstance(entity)) {
                    continue;
                }

                if ((Boolean) isRemovedMethod.invoke(entity)) {
                    continue;
                }

                if (!(Boolean) isAliveMethod.invoke(entity)) {
                    continue;
                }

                UUID uuid = (UUID) uuidField.get(entity);

                if (uuid == null) {
                    continue;
                }

                Object entityType = getTypeMethod.invoke(entity);

                Object registryKey = getRegistryKeyMethod.invoke(
                    entityTypeRegistry,
                    entityType
                );

                if (registryKey == null) {
                    continue;
                }

                String type = registryKey.toString();

                if (!type.matches("[a-z0-9_.-]+:[a-z0-9_./-]+")) {
                    throw new IllegalStateException(
                        "Invalid entity registry key: " + type
                    );
                }

                MobState state = new MobState(
                    uuid,
                    type,
                    ((Number) getXMethod.invoke(entity)).doubleValue(),
                    ((Number) getYMethod.invoke(entity)).doubleValue(),
                    ((Number) getZMethod.invoke(entity)).doubleValue(),
                    ((Number) getYawMethod.invoke(entity)).floatValue(),
                    ((Number) getPitchMethod.invoke(entity)).floatValue(),
                    ((Number) getHealthMethod.invoke(entity)).floatValue(),
                    ((Number) getMaxHealthMethod.invoke(entity)).floatValue()
                );

                if (!state.isFinite()) {
                    System.err.println(
                        "[MonsterSync][INVALID_STATE] Non-finite values; " +
                        "skipping uuid=" + uuid
                    );
                    continue;
                }

                current.put(uuid, state);
                sampled++;

                String outboundJson = null;
                long outboundSeq = 0L;
                String outboundType = null;
                boolean retry = false;

                long nowNanos = System.nanoTime();

                synchronized (STATE_LOCK) {
                    PendingMonster item = pending.get(uuid);
                    MobState confirmed = previous.get(uuid);

                    if (item != null) {
                        if (!state.sameAs(item.state)) {
                            // Replace an obsolete pending state immediately.
                            // Even if state returns to the confirmed value,
                            // send a compensating update: the old packet may
                            // already be in flight.
                            String messageType =
                                confirmed == null
                                    ? "monster_spawn"
                                    : "monster_update";

                            long seq = nextSequence();

                            item = new PendingMonster(
                                state,
                                seq,
                                messageType,
                                nowNanos
                            );

                            // Serialize before committing the new pending state.
                            outboundJson = state.toJson(messageType, seq);

                            pending.put(uuid, item);

                            outboundSeq = seq;
                            outboundType = messageType;

                            if (confirmed != null) {
                                changed++;
                            }
                        } else if (
                            nowNanos - item.lastSentAtNanos >=
                            retryDelayNanos(item.attempts)
                        ) {
                            item.lastSentAtNanos = nowNanos;
                            item.attempts++;

                            outboundJson = item.state.toJson(
                                item.messageType,
                                item.seq
                            );

                            outboundSeq = item.seq;
                            outboundType = item.messageType;
                            retry = true;
                        }
                    } else if (
                        confirmed == null || !state.sameAs(confirmed)
                    ) {
                        String messageType =
                            confirmed == null
                                ? "monster_spawn"
                                : "monster_update";

                        long seq = nextSequence();

                        item = new PendingMonster(
                            state,
                            seq,
                            messageType,
                            nowNanos
                        );

                        // Serialize before installing the pending state.
                        outboundJson = state.toJson(messageType, seq);

                        // Install pending before writing to the socket.
                        pending.put(uuid, item);

                        outboundSeq = seq;
                        outboundType = messageType;

                        if (confirmed != null) {
                            changed++;
                        }
                    }
                }

                if (outboundJson != null) {
                    boolean sent = BusAgent.sendFabricMonster(outboundJson);

                    if (sent) {
                        System.out.println(
                            "[MonsterSync][" +
                            (retry ? "RETRY" : "SEND") +
                            "] type=" + outboundType +
                            " uuid=" + uuid +
                            " seq=" + outboundSeq
                        );
                    } else {
                        System.out.println(
                            "[MonsterSync][SEND_PENDING] type=" +
                            outboundType +
                            " uuid=" + uuid +
                            " seq=" + outboundSeq
                        );
                    }
                }
            }

            Map<UUID, MobState> confirmedSnapshot;

            synchronized (STATE_LOCK) {
                confirmedSnapshot = new HashMap<>(previous);
            }

            int missing = 0;

            for (Map.Entry<UUID, MobState> entry :
                    confirmedSnapshot.entrySet()) {
                UUID oldUuid = entry.getKey();

                if (!current.containsKey(oldUuid)) {
                    MobState old = entry.getValue();

                    System.out.println(
                        "[MonsterSync][MISSING_FROM_ITERATION] type=" +
                        old.type + " uuid=" + oldUuid
                    );

                    missing++;
                }
            }

            // Missing from entitiesForRendering() is NOT a remove event.

            long now = System.currentTimeMillis();

            if (now - lastSummaryAt >= SUMMARY_INTERVAL_MS) {
                lastSummaryAt = now;

                int confirmedCount;
                int pendingCount;

                synchronized (STATE_LOCK) {
                    confirmedCount = previous.size();
                    pendingCount = pending.size();
                }

                System.out.println(
                    "[MonsterSync][SUMMARY] hostile=" + sampled +
                    " changed=" + changed +
                    " missing=" + missing +
                    " confirmed=" + confirmedCount +
                    " pending=" + pendingCount
                );

                int shown = 0;

                for (MobState state : current.values()) {
                    if (shown++ >= 3) {
                        break;
                    }

                    System.out.println(
                        "[MonsterSync][SAMPLE] type=" + state.type +
                        " uuid=" + state.uuid +
                        " pos=" + state.position() +
                        " health=" + state.health +
                        "/" + state.maxHealth +
                        " yaw=" + state.yaw +
                        " pitch=" + state.pitch
                    );
                }
            }

        } catch (Throwable error) {
            if (!errorPrinted) {
                errorPrinted = true;

                System.err.println(
                    "[MonsterSync][ERROR] sampling failed:"
                );

                error.printStackTrace(System.err);
            }
        }
    }

    private static void initialize(Object minecraft) throws Exception {
        if (initialized) {
            return;
        }

        ClassLoader loader = minecraft.getClass().getClassLoader();

        // MinecraftClient.world
        levelField = minecraft.getClass().getField("field_1687");

        // ClientLevel -> TransientEntitySectionManager -> EntityGetter
        Class<?> clientLevelClass =
            loader.loadClass("net.minecraft.class_638");

        entityStorageField =
            clientLevelClass.getDeclaredField("field_27734");
        entityStorageField.setAccessible(true);

        Class<?> managerClass =
            loader.loadClass("net.minecraft.class_5582");

        managerGetGetterMethod =
            managerClass.getMethod("method_31866");

        Class<?> getterClass =
            loader.loadClass("net.minecraft.class_5577");

        entityGetterGetAllMethod =
            getterClass.getMethod("method_31803");

        entityGetterGetByUuidMethod =
            getterClass.getMethod("method_31808", UUID.class);

        // Hostile mob hierarchy
        monsterClass =
            loader.loadClass("net.minecraft.class_1588");

        Class<?> entityClass =
            loader.loadClass("net.minecraft.class_1297");

        Class<?> livingEntityClass =
            loader.loadClass("net.minecraft.class_1309");

        // Entity UUID is a field, not the old cG() method.
        uuidField = entityClass.getDeclaredField("field_6021");
        uuidField.setAccessible(true);

        getTypeMethod =
            entityClass.getMethod("method_5864");

        getXMethod =
            entityClass.getMethod("method_23317");

        getYMethod =
            entityClass.getMethod("method_23318");

        getZMethod =
            entityClass.getMethod("method_23321");

        getYawMethod =
            entityClass.getMethod("method_36454");

        getPitchMethod =
            entityClass.getMethod("method_36455");

        isAliveMethod =
            entityClass.getMethod("method_5805");

        isRemovedMethod =
            entityClass.getMethod("method_31481");

        // BuiltInRegistries.ENTITY_TYPE
        Class<?> builtInRegistriesClass =
            loader.loadClass("net.minecraft.class_7923");

        entityTypeRegistry = builtInRegistriesClass
            .getField("field_41177")
            .get(null);

        // Registry.getKey(Object)
        Class<?> registryClass =
            loader.loadClass("net.minecraft.class_2378");

        getRegistryKeyMethod = registryClass.getMethod(
            "method_10221",
            Object.class
        );

        // LivingEntity health methods
        getHealthMethod =
            livingEntityClass.getMethod("method_6032");

        getMaxHealthMethod =
            livingEntityClass.getMethod("method_6063");

        initialized = true;

        System.out.println(
            "[MonsterSync][INIT] SUCCESS: " +
            "level=" + levelField.getName() +
            ", storage=" + entityStorageField.getName() +
            ", getter=" + managerGetGetterMethod.getName() +
            ", getAll=" + entityGetterGetAllMethod.getName() +
            ", getByUuid=" + entityGetterGetByUuidMethod.getName() +
            ", uuid=" + uuidField.getName() +
            ", registry=" + registryClass.getName() +
            ", hostileBase=" + monsterClass.getName()
        );
    }

    private static final class MobState {
        final UUID uuid;
        final String type;

        final double x;
        final double y;
        final double z;

        final float yaw;
        final float pitch;

        final float health;
        final float maxHealth;

        MobState(
            UUID uuid,
            String type,
            double x,
            double y,
            double z,
            float yaw,
            float pitch,
            float health,
            float maxHealth
        ) {
            this.uuid = uuid;
            this.type = type;
            this.x = x;
            this.y = y;
            this.z = z;
            this.yaw = yaw;
            this.pitch = pitch;
            this.health = health;
            this.maxHealth = maxHealth;
        }

        boolean isFinite() {
            return Double.isFinite(x)
                && Double.isFinite(y)
                && Double.isFinite(z)
                && Float.isFinite(yaw)
                && Float.isFinite(pitch)
                && Float.isFinite(health)
                && Float.isFinite(maxHealth);
        }

        boolean sameAs(MobState other) {
            return type.equals(other.type)
                && Double.compare(x, other.x) == 0
                && Double.compare(y, other.y) == 0
                && Double.compare(z, other.z) == 0
                && Float.compare(yaw, other.yaw) == 0
                && Float.compare(pitch, other.pitch) == 0
                && Float.compare(health, other.health) == 0
                && Float.compare(maxHealth, other.maxHealth) == 0;
        }

        String toJson(String messageType, long seq) {
            if (!isFinite()) {
                throw new IllegalStateException(
                    "Non-finite monster state: " + uuid
                );
            }

            return "{\"to\":\"neoforge\"," +
                "\"type\":\"" + messageType + "\"," +
                "\"uuid\":\"" + uuid + "\"," +
                "\"seq\":" + seq + "," +
                "\"entity_type\":\"" + type + "\"," +
                "\"x\":" + Double.toString(x) + "," +
                "\"y\":" + Double.toString(y) + "," +
                "\"z\":" + Double.toString(z) + "," +
                "\"yaw\":" + Float.toString(yaw) + "," +
                "\"pitch\":" + Float.toString(pitch) + "," +
                "\"health\":" + Float.toString(health) + "," +
                "\"max_health\":" + Float.toString(maxHealth) +
                "}";
        }

        String position() {
            return "(" + x + ", " + y + ", " + z + ")";
        }
    }
}
