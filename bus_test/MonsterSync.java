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
    private static Method entitiesForRenderingMethod;

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

    private static final Map<UUID, MobState> previous =
        new HashMap<>();

    private MonsterSync() {
    }

    static void sample(Object minecraft) {
        if (minecraft == null) {
            return;
        }

        // M1.1: independently limit hostile-mob sampling to 5Hz.
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
                if (!previous.isEmpty()) {
                    System.out.println(
                        "[MonsterSync][WORLD] ClientLevel unavailable; " +
                        previous.size() + " tracked mobs cleared"
                    );
                    previous.clear();
                }
                return;
            }

            Object result = entitiesForRenderingMethod.invoke(level);

            if (!(result instanceof Iterable<?>)) {
                throw new IllegalStateException(
                    "ClientLevel.d() did not return Iterable"
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

                MobState old = previous.get(uuid);

                if (old == null || !state.sameAs(old)) {
                    String messageType =
                        old == null ? "monster_spawn" : "monster_update";

                    if (BusAgent.sendFabricMonster(
                        state.toJson(messageType)
                    )) {
                        previous.put(uuid, state);

                        System.out.println(
                            "[MonsterSync][" +
                            (old == null ? "SPAWN" : "UPDATE") +
                            "] type=" + state.type +
                            " uuid=" + state.uuid +
                            " pos=" + state.position()
                        );
                    } else {
                        System.out.println(
                            "[MonsterSync][SEND_PENDING] type=" +
                            messageType + " uuid=" + uuid
                        );
                    }

                    if (old != null) {
                        changed++;
                    }
                }

                current.put(uuid, state);
                sampled++;
            }

            int missing = 0;

            for (UUID oldUuid : previous.keySet()) {
                if (!current.containsKey(oldUuid)) {
                    MobState old = previous.get(oldUuid);

                    System.out.println(
                        "[MonsterSync][MISSING_FROM_ITERATION] type=" +
                        old.type + " uuid=" + oldUuid
                    );

                    missing++;
                }
            }

            // Keep previous as the last successfully transmitted state.
            // Missing from entitiesForRendering() is NOT a remove event.

            long now = System.currentTimeMillis();

            if (now - lastSummaryAt >= SUMMARY_INTERVAL_MS) {
                lastSummaryAt = now;

                System.out.println(
                    "[MonsterSync][SUMMARY] hostile=" + sampled +
                    " changed=" + changed +
                    " missing=" + missing
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

        // ClientWorld entity iteration
        Class<?> clientLevelClass =
            loader.loadClass("net.minecraft.class_638");

        entitiesForRenderingMethod =
            clientLevelClass.getMethod("method_18112");

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
            ", iterator=" + entitiesForRenderingMethod.getName() +
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

        String toJson(String messageType) {
            if (!Double.isFinite(x) ||
                !Double.isFinite(y) ||
                !Double.isFinite(z) ||
                !Float.isFinite(yaw) ||
                !Float.isFinite(pitch) ||
                !Float.isFinite(health) ||
                !Float.isFinite(maxHealth)) {
                throw new IllegalStateException(
                    "Non-finite monster state: " + uuid
                );
            }

            return "{\"to\":\"neoforge\"," +
                "\"type\":\"" + messageType + "\"," +
                "\"uuid\":\"" + uuid + "\"," +
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
