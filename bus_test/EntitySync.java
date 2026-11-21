import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.HashMap;
import java.util.Map;
import java.util.UUID;

/**
 * Fabric-side living-entity discovery and synchronization.
 *
 * Scope:
 * - Discover all non-player LivingEntity instances in the current ClientLevel.
 * - Track entities by UUID.
 * - Sample position, rotation, registry type and health.
 * - Send bounded, sequenced state and removal messages.
 * - Retain pending operations until the matching NeoForge ACK arrives.
 *
 * Wire message names remain monster_* during this compatibility phase.
 * All sampling calls must run on the Minecraft client main thread.
 */
final class EntitySync {
    private static final java.util.regex.Pattern ENTITY_TYPE_PATTERN =
            java.util.regex.Pattern.compile("[a-z0-9_.-]+:[a-z0-9_./-]+");

    private static final long SUMMARY_INTERVAL_MS = 1000L;

    private static final long SAMPLE_INTERVAL_NS = 200_000_000L;
    private static long lastSampleAtNanos;

    private static Field levelField;
    private static Method isOnThreadMethod;
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

    private static Class<?> livingEntityClass;
    private static Class<?> playerClass;
    private static boolean initialized;
    private static boolean errorPrinted;

    private static long lastSummaryAt;

    private static final Object STATE_LOCK = new Object();

    // Only states confirmed by NeoForge belong in previous.
    private static final Map<UUID, EntityState> previous =
        new HashMap<>();

    // One latest unconfirmed state per monster UUID.
    private static final Map<UUID, PendingEntity> pending =
        new HashMap<>();

    private static final int MAX_TRACKED_ENTITIES = 1024;
    private static final int MAX_PENDING_ENTITIES = 256;
    private static final int MAX_ENTITIES_VISITED_PER_SAMPLE = 8192;
    private static final int MAX_SAMPLED_ENTITIES_PER_SAMPLE = 1024;

    // 0 is a permanent exhausted sentinel. Long.MAX_VALUE is issued once.
    private static final java.util.concurrent.atomic.AtomicLong NEXT_SEQUENCE =
        new java.util.concurrent.atomic.AtomicLong(1L);

    private EntitySync() {
    }

    private static long nextSequence() {
        for (;;) {
            long current = NEXT_SEQUENCE.get();

            if (current <= 0L) {
                throw new IllegalStateException(
                    "Monster sequence exhausted"
                );
            }

            long next = current == Long.MAX_VALUE ? 0L : current + 1L;

            if (NEXT_SEQUENCE.compareAndSet(current, next)) {
                return current;
            }
        }
    }

    // Must be called while holding STATE_LOCK.
    private static int trackedCountLocked() {
        int count = previous.size();

        for (UUID uuid : pending.keySet()) {
            if (!previous.containsKey(uuid)) {
                count++;
            }
        }

        return count;
    }

    private static long retryDelayNanos(int attempts) {
        if (attempts <= 1) {
            return 1_000_000_000L;
        }
        if (attempts == 2) {
            return 2_000_000_000L;
        }
        if (attempts <= 4) {
            return 5_000_000_000L;
        }
        return 30_000_000_000L;
    }

    // Called by the DataBus receive thread.
    static void acknowledge(UUID uuid, long seq) {
        if (uuid == null || seq <= 0L) {
            return;
        }

        EntityState confirmed = null;
        String acknowledgedType = null;

        synchronized (STATE_LOCK) {
            PendingEntity item = pending.get(uuid);

            if (item == null || item.seq != seq) {
                BusAgent.logDebug(
                    "[EntitySync][ACK][STALE] uuid=" +
                    uuid + " seq=" + seq
                );
                return;
            }

            confirmed = item.state;
            acknowledgedType = item.messageType;

            if ("entity_remove".equals(item.messageType)) {
                previous.remove(uuid);
            } else {
                if (confirmed == null) {
                    throw new IllegalStateException(
                        "State ACK has null monster state: " + uuid
                    );
                }
                previous.put(uuid, confirmed);
            }

            pending.remove(uuid);
        }

        BusAgent.logDebug(
            "[EntitySync][ACK] type=" + acknowledgedType +
            " uuid=" + uuid +
            " seq=" + seq +
            (confirmed == null ? "" : " pos=" + confirmed.position())
        );
    }

    private static final class PendingEntity {
        final EntityState state;
        final long seq;
        final String messageType;

        long lastSentAtNanos;
        int attempts;

        PendingEntity(
            EntityState state,
            long seq,
            String messageType,
            long nowNanos
        ) {
            this.state = state;
            this.seq = seq;
            this.messageType = messageType;
            // A pending operation is not considered sent until the
            // socket write succeeds.
            this.lastSentAtNanos = 0L;
            this.attempts = 0;
        }
    }

    private static final class OutboundEntity {
        final UUID uuid;
        final PendingEntity item;

        OutboundEntity(UUID uuid, PendingEntity item) {
            if (uuid == null || item == null) {
                throw new IllegalArgumentException(
                    "Invalid outbound monster snapshot");
            }
            this.uuid = uuid;
            this.item = item;
        }
    }

    private static void markSendSucceeded(
            UUID uuid, PendingEntity expected, long sentAtNanos) {
        synchronized (STATE_LOCK) {
            PendingEntity item = pending.get(uuid);
            if (item == null || item != expected) {
                return;
            }

            if (item.attempts < 5) {
                item.attempts++;
            }
            item.lastSentAtNanos = sentAtNanos;
        }
    }

    private static boolean retryDue(PendingEntity item, long nowNanos) {
        if (item.attempts == 0) {
            return true;
        }
        return nowNanos - item.lastSentAtNanos >=
                retryDelayNanos(item.attempts);
    }

    private static String removeJson(UUID uuid, long seq) {
        if (uuid == null || seq <= 0L) {
            throw new IllegalStateException("Invalid monster removal envelope");
        }

        return "{\"to\":\"neoforge\"," +
                "\"type\":\"entity_remove\"," +
                "\"uuid\":\"" + uuid + "\"," +
                "\"seq\":" + seq +
                "}";
    }

    static void sample(Object minecraft) {
        if (minecraft == null) {
            return;
        }

        try {
            initialize(minecraft);

            if (isOnThreadMethod == null ||
                    !Boolean.TRUE.equals(isOnThreadMethod.invoke(minecraft))) {
                throw new IllegalStateException(
                        "EntitySync.sample() must run on the Minecraft client thread");
            }

            long sampleNow = System.nanoTime();

            if (lastSampleAtNanos != 0L &&
                    sampleNow - lastSampleAtNanos < SAMPLE_INTERVAL_NS) {
                return;
            }

            lastSampleAtNanos = sampleNow;

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
                            "[EntitySync][WORLD] ClientLevel unavailable; " +
                            "confirmed=" + clearedPrevious +
                            " pending=" + clearedPending + " cleared");
                }
                return;
            }

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
                        "[EntitySync][WORLD] ClientLevel changed; " +
                        "confirmed=" + clearedPrevious +
                        " pending=" + clearedPending + " cleared");
            }

            lastLevel = level;

            Object storage = entityStorageField.get(level);
            if (storage == null) {
                throw new IllegalStateException(
                        "ClientLevel entity storage is null");
            }

            Object getter = managerGetGetterMethod.invoke(storage);
            if (getter == null) {
                throw new IllegalStateException("EntityGetter is null");
            }

            Object result = entityGetterGetAllMethod.invoke(getter);
            if (!(result instanceof Iterable<?>)) {
                throw new IllegalStateException(
                        "EntityGetter.getAll() did not return Iterable");
            }

            Map<UUID, EntityState> current = new HashMap<>();
            int sampled = 0;
            int changed = 0;
            int visited = 0;
            int capacitySkipped = 0;
            boolean scanComplete = false;

            java.util.List<OutboundEntity> outbound = new java.util.ArrayList<>();

            // A limit is considered truncation only when another element
            // actually exists beyond that limit.
            for (Object entity : (Iterable<?>) result) {
                if (visited >= MAX_ENTITIES_VISITED_PER_SAMPLE) {
                    scanComplete = false;
                    break;
                }
                visited++;

                if (entity == null ||
                        !livingEntityClass.isInstance(entity) ||
                        playerClass.isInstance(entity)) {
                    continue;
                }

                if (sampled >= MAX_SAMPLED_ENTITIES_PER_SAMPLE) {
                    scanComplete = false;
                    break;
                }

                if ((Boolean) isRemovedMethod.invoke(entity) ||
                        !(Boolean) isAliveMethod.invoke(entity)) {
                    continue;
                }

                UUID uuid = (UUID) uuidField.get(entity);
                if (uuid == null) {
                    continue;
                }

                Object entityType = getTypeMethod.invoke(entity);
                Object registryKey = getRegistryKeyMethod.invoke(
                        entityTypeRegistry, entityType);

                if (registryKey == null) {
                    continue;
                }

                String type = registryKey.toString();
                if (!ENTITY_TYPE_PATTERN.matcher(type).matches()) {
                    throw new IllegalStateException(
                            "Invalid entity registry key: " + type);
                }

                EntityState state = new EntityState(
                        uuid,
                        type,
                        ((Number) getXMethod.invoke(entity)).doubleValue(),
                        ((Number) getYMethod.invoke(entity)).doubleValue(),
                        ((Number) getZMethod.invoke(entity)).doubleValue(),
                        ((Number) getYawMethod.invoke(entity)).floatValue(),
                        ((Number) getPitchMethod.invoke(entity)).floatValue(),
                        ((Number) getHealthMethod.invoke(entity)).floatValue(),
                        ((Number) getMaxHealthMethod.invoke(entity)).floatValue());

                if (!state.isFinite()) {
                    BusAgent.logDebug(
                            "[EntitySync][INVALID_STATE] uuid=" + uuid);
                    continue;
                }

                if (current.putIfAbsent(uuid, state) != null) {
                    BusAgent.logDebug(
                            "[EntitySync][DUPLICATE_UUID] uuid=" + uuid);
                    continue;
                }

                sampled++;

                long nowNanos = System.nanoTime();

                synchronized (STATE_LOCK) {
                    PendingEntity item = pending.get(uuid);
                    EntityState confirmed = previous.get(uuid);

                    if (item == null &&
                            (pending.size() >= MAX_PENDING_ENTITIES ||
                             (confirmed == null &&
                              trackedCountLocked() >= MAX_TRACKED_ENTITIES))) {
                        capacitySkipped++;
                        continue;
                    }

                    if (item != null &&
                            "entity_remove".equals(item.messageType)) {
                        // The monster reappeared before its removal was
                        // acknowledged. Supersede the removal with state.
                        String typeToSend = confirmed == null
                                ? "entity_spawn" : "entity_update";

                        item = new PendingEntity(
                                state, nextSequence(), typeToSend, nowNanos);
                        item.state.toJson(typeToSend, item.seq);
                        pending.put(uuid, item);
                        outbound.add(new OutboundEntity(uuid, item));

                        if (confirmed != null) {
                            changed++;
                        }
                        continue;
                    }

                    if (item != null) {
                        if (item.state == null ||
                                !state.sameAs(item.state)) {
                            String typeToSend = confirmed == null
                                    ? "entity_spawn" : "entity_update";

                            item = new PendingEntity(
                                    state, nextSequence(), typeToSend, nowNanos);

                            // Validate serialization before committing.
                            item.state.toJson(typeToSend, item.seq);
                            pending.put(uuid, item);
                            outbound.add(new OutboundEntity(uuid, item));

                            if (confirmed != null) {
                                changed++;
                            }
                        } else if (retryDue(item, nowNanos)) {
                            outbound.add(new OutboundEntity(uuid, item));
                        }
                    } else if (confirmed == null ||
                            !state.sameAs(confirmed)) {
                        String typeToSend = confirmed == null
                                ? "entity_spawn" : "entity_update";

                        item = new PendingEntity(
                                state, nextSequence(), typeToSend, nowNanos);

                        item.state.toJson(typeToSend, item.seq);
                        pending.put(uuid, item);
                        outbound.add(new OutboundEntity(uuid, item));

                        if (confirmed != null) {
                            changed++;
                        }
                    }
                }
            }

            // Reconciliation is permitted only after the iterator completed
            // without hitting either safety limit or throwing an exception.
            // Reaching exactly the limit is safe if there was no extra item.
            if (visited < MAX_ENTITIES_VISITED_PER_SAMPLE &&
                    sampled < MAX_SAMPLED_ENTITIES_PER_SAMPLE) {
                scanComplete = true;
            }

            int missing = 0;

            if (scanComplete) {
                synchronized (STATE_LOCK) {
                    for (Map.Entry<UUID, EntityState> entry :
                            previous.entrySet()) {
                        UUID uuid = entry.getKey();

                        if (current.containsKey(uuid)) {
                            continue;
                        }

                        missing++;

                        PendingEntity existing = pending.get(uuid);

                        if (existing != null &&
                                "entity_remove".equals(existing.messageType)) {
                            continue;
                        }

                        if (existing != null && existing.state == null) {
                            continue;
                        }

                        if (pending.size() >= MAX_PENDING_ENTITIES &&
                                existing == null) {
                            capacitySkipped++;
                            continue;
                        }

                        PendingEntity removal = new PendingEntity(
                                null,
                                nextSequence(),
                                "entity_remove",
                                System.nanoTime());

                        // Commit only after the envelope is valid.
                        removeJson(uuid, removal.seq);
                        pending.put(uuid, removal);
                        outbound.add(new OutboundEntity(uuid, removal));
                    }

                    // A pending spawn/update that has never been confirmed
                    // is intentionally retained when absent from the scan.
                    // If its ACK later arrives, the next complete scan
                    // generates the corresponding remove operation.
                }
            }

            for (OutboundEntity snapshot : outbound) {
                UUID uuid = snapshot.uuid;
                PendingEntity item = snapshot.item;

                // Skip operations already ACKed or superseded.
                boolean stillCurrent;
                synchronized (STATE_LOCK) {
                    stillCurrent = pending.get(uuid) == item;
                }
                if (!stillCurrent) {
                    continue;
                }

                String json = "entity_remove".equals(item.messageType)
                        ? removeJson(uuid, item.seq)
                        : item.state.toJson(item.messageType, item.seq);

                boolean sent = BusAgent.sendFabricMonster(json);
                if (sent) {
                    markSendSucceeded(uuid, item, System.nanoTime());
                    BusAgent.logDebug(
                            "[EntitySync][SEND] type=" + item.messageType +
                            " uuid=" + uuid + " seq=" + item.seq);
                }
            }

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
                        "[EntitySync][SUMMARY] hostile=" + sampled +
                        " visited=" + visited +
                        " complete=" + scanComplete +
                        " changed=" + changed +
                        " missing=" + missing +
                        " capacitySkipped=" + capacitySkipped +
                        " confirmed=" + confirmedCount +
                        " pending=" + pendingCount);
            }

        } catch (Throwable error) {
            if (!errorPrinted) {
                errorPrinted = true;
                System.err.println(
                        "[EntitySync][ERROR] sampling failed:");
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
        isOnThreadMethod = minecraft.getClass().getMethod("method_18854");

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

        // Living entities, excluding all player entities.
        Class<?> entityClass =
            loader.loadClass("net.minecraft.class_1297");

        livingEntityClass =
            loader.loadClass("net.minecraft.class_1309");

        playerClass =
            loader.loadClass("net.minecraft.class_1657");

        Class<?> livingClass = livingEntityClass;

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
            livingClass.getMethod("method_6032");

        getMaxHealthMethod =
            livingClass.getMethod("method_6063");

        initialized = true;

        System.out.println(
            "[EntitySync][INIT] SUCCESS: " +
            "level=" + levelField.getName() +
            ", storage=" + entityStorageField.getName() +
            ", getter=" + managerGetGetterMethod.getName() +
            ", getAll=" + entityGetterGetAllMethod.getName() +
            ", getByUuid=" + entityGetterGetByUuidMethod.getName() +
            ", uuid=" + uuidField.getName() +
            ", registry=" + registryClass.getName() +
            ", livingBase=" + livingEntityClass.getName() +
            ", excludedPlayerBase=" + playerClass.getName()
        );
    }

    private static final class EntityState {
        final UUID uuid;
        final String type;

        final double x;
        final double y;
        final double z;

        final float yaw;
        final float pitch;

        final float health;
        final float maxHealth;

        EntityState(
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

        boolean sameAs(EntityState other) {
            if (other == null) {
                return false;
            }

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
            if (!isFinite() ||
                    uuid == null ||
                    seq <= 0L ||
                    !("entity_spawn".equals(messageType) ||
                      "entity_update".equals(messageType)) ||
                    !ENTITY_TYPE_PATTERN.matcher(type).matches() ||
                    maxHealth < 0.0f ||
                    health < 0.0f) {
                throw new IllegalStateException(
                    "Invalid monster state or outbound envelope: " + uuid
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
