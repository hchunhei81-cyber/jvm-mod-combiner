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
    private static Method getTypeDescriptionMethod;
    private static Method getUuidMethod;
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

                UUID uuid = (UUID) getUuidMethod.invoke(entity);

                if (uuid == null) {
                    continue;
                }

                Object entityType = getTypeMethod.invoke(entity);
                String type = String.valueOf(
                    getTypeDescriptionMethod.invoke(entityType)
                );

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

                if (old == null) {
                    System.out.println(
                        "[MonsterSync][FOUND] type=" + state.type +
                        " uuid=" + state.uuid +
                        " pos=" + state.position()
                    );
                } else if (!state.sameAs(old)) {
                    changed++;
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

            previous.clear();
            previous.putAll(current);

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

        levelField = minecraft.getClass().getField("s");

        Class<?> clientLevelClass = loader.loadClass("gga");
        entitiesForRenderingMethod =
            clientLevelClass.getMethod("d");

        monsterClass = loader.loadClass("clz");

        Class<?> entityClass = loader.loadClass("bum");
        Class<?> livingEntityClass = loader.loadClass("bvi");

        getTypeMethod = entityClass.getMethod("aq");
        getUuidMethod = entityClass.getMethod("cG");
        getXMethod = entityClass.getMethod("dA");
        getYMethod = entityClass.getMethod("dC");
        getZMethod = entityClass.getMethod("dG");
        getYawMethod = entityClass.getMethod("dL");
        getPitchMethod = entityClass.getMethod("dN");
        isAliveMethod = entityClass.getMethod("bL");
        isRemovedMethod = entityClass.getMethod("dQ");

        Class<?> entityTypeClass = loader.loadClass("but");
        getTypeDescriptionMethod =
            entityTypeClass.getMethod("g");

        getHealthMethod = livingEntityClass.getMethod("eD");
        getMaxHealthMethod = livingEntityClass.getMethod("eS");

        initialized = true;

        System.out.println(
            "[MonsterSync][INIT] SUCCESS: " +
            "level=" + levelField.getName() +
            ", iterator=" + entitiesForRenderingMethod.getName() +
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

        String position() {
            return "(" + x + ", " + y + ", " + z + ")";
        }
    }
}
