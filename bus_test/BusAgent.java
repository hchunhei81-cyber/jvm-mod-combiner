import java.io.*;
import java.net.*;
import java.lang.instrument.*;
import java.lang.reflect.*;
import java.util.*;
import java.nio.channels.Channels;
import java.nio.channels.SocketChannel;

public class BusAgent {
    private static final java.util.regex.Pattern UUID_PATTERN = java.util.regex.Pattern.compile(
            "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-" +
                    "[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-" +
                    "[0-9a-fA-F]{12}");

    private static final java.util.regex.Pattern ENTITY_TYPE_PATTERN = java.util.regex.Pattern
            .compile("[a-z0-9_.-]+:[a-z0-9_./-]+");

    private static final java.util.Set<String> ACK_FIELDS = java.util.Set.of("from", "to", "type", "uuid", "seq");

    private static final java.util.Set<String> PLAYER_FIELDS = java.util.Set.of(
            "from", "to", "type",
            "x", "y", "z", "yaw", "pitch", "sprint");

    private static final java.util.Set<String> ENTITY_FIELDS = java.util.Set.of(
            "from", "to", "type", "uuid", "seq", "entity_type",
            "x", "y", "z", "yaw", "pitch", "health", "max_health");

    private static final java.util.Set<String> ENTITY_REMOVE_FIELDS = java.util.Set.of("from", "to", "type", "uuid",
            "seq");

    private static final java.util.concurrent.atomic.AtomicLong LAST_INVALID_MESSAGE_LOG_AT = new java.util.concurrent.atomic.AtomicLong(
            0L);

    private static void logRejectedMessage(String reason) {
        long now = System.currentTimeMillis();
        long previous = LAST_INVALID_MESSAGE_LOG_AT.get();

        if (now - previous >= 1000L &&
                LAST_INVALID_MESSAGE_LOG_AT.compareAndSet(previous, now)) {
            System.err.println(
                    "[BusAgent:" + myName +
                            "][DataBus][REJECTED] " + reason);
        }
    }

    /**
     * Reads one newline-terminated message without allocating an
     * unbounded String or accepting a truncated final message.
     */
    private static byte[] readBoundedLine(java.io.InputStream input)
            throws java.io.IOException {
        java.io.ByteArrayOutputStream buffer = new java.io.ByteArrayOutputStream(1024);

        while (true) {
            int value = input.read();

            if (value < 0) {
                if (buffer.size() == 0) {
                    return null;
                }
                throw new java.io.IOException(
                        "Truncated DataBus message without newline");
            }

            if (value == '\n') {
                return buffer.toByteArray();
            }

            if (buffer.size() >= StrictJson.MAX_MESSAGE_BYTES) {
                throw new java.io.IOException(
                        "DataBus message exceeds 64 KiB");
            }

            buffer.write(value);
        }
    }

    /**
     * Validates direction, exact field set, and every field type.
     * Rust injects "from"; Java must never manufacture that identity.
     */
    private static String validateInboundMessage(
            java.util.Map<String, Object> message,
            String expectedPeer,
            String localRole) {

        String type = StrictJson.string(message, "type");
        String from = StrictJson.string(message, "from");
        String to = StrictJson.string(message, "to");

        if (!expectedPeer.equals(from) || !localRole.equals(to)) {
            throw new IllegalArgumentException(
                    "Unexpected DataBus message direction");
        }

        switch (type) {
            case "entity_ack":
                if (!"fabric".equals(localRole) ||
                        !"neoforge".equals(expectedPeer)) {
                    throw new IllegalArgumentException(
                            "entity_ack received by wrong role");
                }

                StrictJson.requireKeys(message, ACK_FIELDS, ACK_FIELDS);

                String ackUuid = StrictJson.string(message, "uuid");
                if (!UUID_PATTERN.matcher(ackUuid).matches()) {
                    throw new IllegalArgumentException("Invalid ACK UUID");
                }

                StrictJson.positiveLong(message, "seq");
                break;

            case "entity_remove":
                if (!"neoforge".equals(localRole) ||
                        !"fabric".equals(expectedPeer)) {
                    throw new IllegalArgumentException(
                            "entity_remove received by wrong role");
                }

                StrictJson.requireKeys(
                        message,
                        ENTITY_REMOVE_FIELDS,
                        ENTITY_REMOVE_FIELDS);

                String removeUuid = StrictJson.string(message, "uuid");

                if (!UUID_PATTERN.matcher(removeUuid).matches()) {
                    throw new IllegalArgumentException(
                            "Invalid monster removal UUID");
                }

                StrictJson.positiveLong(message, "seq");
                break;

            case "player_move":
                if (!"neoforge".equals(localRole) ||
                        !"fabric".equals(expectedPeer)) {
                    throw new IllegalArgumentException(
                            "player_move received by wrong role");
                }

                StrictJson.requireKeys(
                        message, PLAYER_FIELDS, PLAYER_FIELDS);

                StrictJson.finiteDouble(message, "x");
                StrictJson.finiteDouble(message, "y");
                StrictJson.finiteDouble(message, "z");
                StrictJson.finiteFloat(message, "yaw");
                StrictJson.finiteFloat(message, "pitch");
                StrictJson.bool(message, "sprint");
                break;

            case "entity_spawn":
            case "entity_update":
                if (!"neoforge".equals(localRole) ||
                        !"fabric".equals(expectedPeer)) {
                    throw new IllegalArgumentException(
                            "Monster state received by wrong role");
                }

                StrictJson.requireKeys(
                        message, ENTITY_FIELDS, ENTITY_FIELDS);

                String uuid = StrictJson.string(message, "uuid");
                if (!UUID_PATTERN.matcher(uuid).matches()) {
                    throw new IllegalArgumentException("Invalid monster UUID");
                }

                StrictJson.positiveLong(message, "seq");

                String entityType = StrictJson.string(message, "entity_type");
                if (!ENTITY_TYPE_PATTERN.matcher(entityType).matches()) {
                    throw new IllegalArgumentException(
                            "Invalid entity registry identifier");
                }

                StrictJson.finiteDouble(message, "x");
                StrictJson.finiteDouble(message, "y");
                StrictJson.finiteDouble(message, "z");
                StrictJson.finiteFloat(message, "yaw");
                StrictJson.finiteFloat(message, "pitch");

                float health = StrictJson.finiteFloat(message, "health");
                float maxHealth = StrictJson.finiteFloat(message, "max_health");

                if (health < 0.0f || maxHealth < 0.0f) {
                    throw new IllegalArgumentException(
                            "Negative monster health");
                }
                break;

            default:
                throw new IllegalArgumentException(
                        "Unsupported inbound message type");
        }

        return type;
    }

    private static final java.util.Set<String> ROUTED_FIELDS =
            java.util.Set.of("routed", "from", "to");
    private static final java.util.Set<String> ERROR_FIELDS =
            java.util.Set.of("error");
    private static final java.util.concurrent.atomic.AtomicLong
            LAST_SERVER_ERROR_LOG_AT =
            new java.util.concurrent.atomic.AtomicLong(0L);

    /**
     * Handles Rust DataBus control frames before peer game-message validation.
     * Returns false only when the frame is an ordinary peer game message.
     */
    private static boolean handleInboundControlMessage(
            java.util.Map<String, Object> message,
            String expectedPeer,
            String localRole) {

        boolean hasRouted = message.containsKey("routed");
        boolean hasError = message.containsKey("error");

        if (!hasRouted && !hasError) {
            return false;
        }

        if (hasRouted == hasError) {
            throw new IllegalArgumentException(
                    "Ambiguous DataBus control frame");
        }

        if (hasRouted) {
            StrictJson.requireKeys(
                    message, ROUTED_FIELDS, ROUTED_FIELDS);

            if (!StrictJson.bool(message, "routed")) {
                throw new IllegalArgumentException(
                        "DataBus route confirmation must be true");
            }

            String from = StrictJson.string(message, "from");
            String to = StrictJson.string(message, "to");

            if (!localRole.equals(from)) {
                throw new IllegalArgumentException(
                        "DataBus route confirmation has wrong sender");
            }

            if (!"*".equals(to) && !expectedPeer.equals(to)) {
                throw new IllegalArgumentException(
                        "DataBus route confirmation has wrong target");
            }

            logDebug(
                    "[BusAgent:" + localRole +
                    "][DataBus][ROUTED] to=" + to);
            return true;
        }

        StrictJson.requireKeys(message, ERROR_FIELDS, ERROR_FIELDS);
        String errorCode = StrictJson.string(message, "error");

        if (!errorCode.matches("[a-zA-Z0-9_.-]{1,128}")) {
            throw new IllegalArgumentException(
                    "Invalid DataBus server error code");
        }

        long now = System.currentTimeMillis();
        long previous = LAST_SERVER_ERROR_LOG_AT.get();

        if (now - previous >= 1000L &&
                LAST_SERVER_ERROR_LOG_AT.compareAndSet(previous, now)) {
            System.err.println(
                    "[BusAgent:" + localRole +
                    "][DataBus][SERVER_ERROR] " + errorCode);
        }

        return true;
    }

    // Default INFO keeps routine per-message diagnostics out of the console.
    private static final String CONFIGURED_LOG_LEVEL = System.getenv("JVM_COMBINER_LOG_LEVEL");
    private static final boolean DEBUG_LOGS = CONFIGURED_LOG_LEVEL != null &&
            "DEBUG".equalsIgnoreCase(CONFIGURED_LOG_LEVEL.trim());

    static void logDebug(String message) {
        if (DEBUG_LOGS) {
            System.out.println(message);
        }
    }

    static volatile PrintWriter out;
    static String myName;
    static Instrumentation inst;

    // Fabric
    static Object fabricMcInstance = null;
    static Field fabricPlayerField = null;
    static volatile boolean fabricStructureDumped = false;

    static volatile java.lang.reflect.Method fabricExecuteMethod = null;

    // Cached Fabric Loader reflection bindings.
    static volatile Class<?> fabricLoaderClass = null;
    static volatile Method fabricGetInstanceMethod = null;
    static volatile Method fabricGetGameInstanceMethod = null;

    enum FabricBindingState {
        UNINITIALIZED,
        READY,
        FAILED
    }

    static volatile FabricBindingState fabricBindingState = FabricBindingState.UNINITIALIZED;
    static final java.util.concurrent.atomic.AtomicBoolean fabricSamplePending = new java.util.concurrent.atomic.AtomicBoolean(
            false);
    static volatile String lastFabricState = null;
    static volatile long lastFabricSendAt = 0L;
    static volatile boolean fabricSampleErrorPrinted = false;

    static Method getXMethod, getYMethod, getZMethod;
    static Method getYRotMethod, getXRotMethod, isSprintingMethod;
    static volatile long lastFabricBindAttemptAt = 0L;

    // NeoForge
    static volatile Object nfMcInstance = null;
    static volatile Class<?> nfMcClass = null;
    static volatile Method nfGetInstanceMethod = null;
    static volatile Method nfExecuteMethod = null;
    static volatile Field nfLevelField = null;
    static volatile Field nfPlayerField = null;

    // Immutable, ClassLoader-scoped NeoForge player reflection bindings.
    static volatile NeoForgePlayerBindings nfPlayerBindings = null;

    static final class NeoForgePlayerBindings {
        final ClassLoader loader;

        final Class<?> levelClass;
        final Class<?> playerClass;
        final Class<?> profileClass;
        final Class<?> entityClass;

        final Constructor<?> profileConstructor;
        final Constructor<?> playerConstructor;

        final Method getEntity;
        final Method setId;
        final Method setUUID;
        final Method setPos;
        final Method setOldPosAndRot;
        final Method addEntity;

        // Optional methods preserve the previous per-method failure behavior.
        final Method setYRot;
        final Method setXRot;
        final Method setYHeadRot;
        final Method setYBodyRot;
        final Method setSprinting;

        NeoForgePlayerBindings(
                ClassLoader loader,
                Class<?> levelClass,
                Class<?> playerClass,
                Class<?> profileClass,
                Class<?> entityClass,
                Constructor<?> profileConstructor,
                Constructor<?> playerConstructor,
                Method getEntity,
                Method setId,
                Method setUUID,
                Method setPos,
                Method setOldPosAndRot,
                Method addEntity,
                Method setYRot,
                Method setXRot,
                Method setYHeadRot,
                Method setYBodyRot,
                Method setSprinting) {

            this.loader = loader;
            this.levelClass = levelClass;
            this.playerClass = playerClass;
            this.profileClass = profileClass;
            this.entityClass = entityClass;
            this.profileConstructor = profileConstructor;
            this.playerConstructor = playerConstructor;
            this.getEntity = getEntity;
            this.setId = setId;
            this.setUUID = setUUID;
            this.setPos = setPos;
            this.setOldPosAndRot = setOldPosAndRot;
            this.addEntity = addEntity;
            this.setYRot = setYRot;
            this.setXRot = setXRot;
            this.setYHeadRot = setYHeadRot;
            this.setYBodyRot = setYBodyRot;
            this.setSprinting = setSprinting;
        }
    }

    static Method optionalPlayerMethod(
            Class<?> type, String name, Class<?>... parameterTypes) {
        try {
            return type.getMethod(name, parameterTypes);
        } catch (Throwable ignored) {
            return null;
        }
    }

    static synchronized NeoForgePlayerBindings getNeoForgePlayerBindings(
            ClassLoader loader) throws Exception {

        NeoForgePlayerBindings cached = nfPlayerBindings;

        if (cached != null && cached.loader == loader) {
            return cached;
        }

        // Build locally. Publish only after every required binding succeeds.
        Class<?> levelClass = Class.forName(
                "net.minecraft.client.multiplayer.ClientLevel", true, loader);
        Class<?> playerClass = Class.forName(
                "net.minecraft.client.player.RemotePlayer", true, loader);
        Class<?> profileClass = Class.forName(
                "com.mojang.authlib.GameProfile", true, loader);
        Class<?> entityClass = Class.forName(
                "net.minecraft.world.entity.Entity", true, loader);
        Class<?> entityTypeClass = Class.forName(
                "net.minecraft.world.entity.EntityType", true, loader);
        Class<?> registryApiClass = Class.forName(
                "net.minecraft.core.Registry", true, loader);

        Constructor<?> profileConstructor = profileClass.getConstructor(java.util.UUID.class, String.class);
        Constructor<?> playerConstructor = playerClass.getConstructor(levelClass, profileClass);

        Method getEntity = levelClass.getMethod("getEntity", int.class);
        Method setId = entityClass.getMethod("setId", int.class);
        Method setUUID = entityClass.getMethod("setUUID", java.util.UUID.class);
        Method setPos = entityClass.getMethod(
                "setPos", double.class, double.class, double.class);
        Method setOldPosAndRot = entityClass.getMethod("setOldPosAndRot");
        Method addEntity = levelClass.getMethod("addEntity", entityClass);

        Method setYRot = optionalPlayerMethod(playerClass, "setYRot", float.class);
        Method setXRot = optionalPlayerMethod(playerClass, "setXRot", float.class);
        Method setYHeadRot = optionalPlayerMethod(playerClass, "setYHeadRot", float.class);
        Method setYBodyRot = optionalPlayerMethod(playerClass, "setYBodyRot", float.class);
        Method setSprinting = optionalPlayerMethod(playerClass, "setSprinting", boolean.class);

        NeoForgePlayerBindings result = new NeoForgePlayerBindings(
                loader,
                levelClass,
                playerClass,
                profileClass,
                entityClass,
                profileConstructor,
                playerConstructor,
                getEntity,
                setId,
                setUUID,
                setPos,
                setOldPosAndRot,
                addEntity,
                setYRot,
                setXRot,
                setYHeadRot,
                setYBodyRot,
                setSprinting);

        nfPlayerBindings = result;
        return result;
    }

    // Immutable, ClassLoader-scoped NeoForge monster reflection bindings.
    static volatile NeoForgeEntityBindings nfEntityBindings = null;

    static final class NeoForgeEntityBindings {
        final ClassLoader loader;

        final Class<?> entityClass;
        final Class<?> livingClass;
        final Class<?> playerClass;
        final Class<?> levelClass;
        final Class<?> clientLevelClass;
        final Class<?> entityTypeClass;
        final Class<?> registryClass;
        final Class<?> registryApiClass;
        final Class<?> resourceLocationClass;
        final Class<?> spawnReasonClass;

        final Field entityTypeRegistryField;

        final Method entityIsRemoved;
        final Method resourceLocationParse;
        final Method registryGetOptional;
        final Method entityTypeCreate;
        final Method clientLevelGetEntity;
        final Method entityGetId;
        final Method entitySetId;
        final Method entitySetUUID;
        final Method entitySetPos;
        final Method entitySetYRot;
        final Method entitySetXRot;
        final Method entitySetYHeadRot;
        final Method entitySetYBodyRot;
        final Method entitySetOldPosAndRot;
        final Method clientLevelAddEntity;
        final Method livingSetHealth;

        NeoForgeEntityBindings(
                ClassLoader loader,
                Class<?> entityClass,
                Class<?> livingClass,
                Class<?> playerClass,
                Class<?> levelClass,
                Class<?> clientLevelClass,
                Class<?> entityTypeClass,
                Class<?> registryClass,
                Class<?> registryApiClass,
                Class<?> resourceLocationClass,
                Class<?> spawnReasonClass,
                Field entityTypeRegistryField,
                Method entityIsRemoved,
                Method resourceLocationParse,
                Method registryGetOptional,
                Method entityTypeCreate,
                Method clientLevelGetEntity,
                Method entityGetId,
                Method entitySetId,
                Method entitySetUUID,
                Method entitySetPos,
                Method entitySetYRot,
                Method entitySetXRot,
                Method entitySetYHeadRot,
                Method entitySetYBodyRot,
                Method entitySetOldPosAndRot,
                Method clientLevelAddEntity,
                Method livingSetHealth) {

            this.loader = loader;
            this.entityClass = entityClass;
            this.livingClass = livingClass;
            this.playerClass = playerClass;
            this.levelClass = levelClass;
            this.clientLevelClass = clientLevelClass;
            this.entityTypeClass = entityTypeClass;
            this.registryClass = registryClass;
            this.registryApiClass = registryApiClass;
            this.resourceLocationClass = resourceLocationClass;
            this.spawnReasonClass = spawnReasonClass;
            this.entityTypeRegistryField = entityTypeRegistryField;
            this.entityIsRemoved = entityIsRemoved;
            this.resourceLocationParse = resourceLocationParse;
            this.registryGetOptional = registryGetOptional;
            this.entityTypeCreate = entityTypeCreate;
            this.clientLevelGetEntity = clientLevelGetEntity;
            this.entityGetId = entityGetId;
            this.entitySetId = entitySetId;
            this.entitySetUUID = entitySetUUID;
            this.entitySetPos = entitySetPos;
            this.entitySetYRot = entitySetYRot;
            this.entitySetXRot = entitySetXRot;
            this.entitySetYHeadRot = entitySetYHeadRot;
            this.entitySetYBodyRot = entitySetYBodyRot;
            this.entitySetOldPosAndRot = entitySetOldPosAndRot;
            this.clientLevelAddEntity = clientLevelAddEntity;
            this.livingSetHealth = livingSetHealth;
        }
    }

    static NeoForgeEntityBindings getNeoForgeEntityBindings(
            ClassLoader loader) throws Exception {

        NeoForgeEntityBindings cached = nfEntityBindings;
        if (cached != null && cached.loader == loader) {
            return cached;
        }

        synchronized (BusAgent.class) {
            cached = nfEntityBindings;
            if (cached != null && cached.loader == loader) {
                return cached;
            }

            // Build locally. Publish only after every required lookup succeeds.
            Class<?> entityClass = Class.forName(
                    "net.minecraft.world.entity.Entity", true, loader);
            Class<?> livingClass = Class.forName(
                    "net.minecraft.world.entity.LivingEntity", true, loader);
            Class<?> playerClass = Class.forName(
                    "net.minecraft.world.entity.player.Player", true, loader);
            Class<?> levelClass = Class.forName(
                    "net.minecraft.world.level.Level", true, loader);
            Class<?> clientLevelClass = Class.forName(
                    "net.minecraft.client.multiplayer.ClientLevel", true, loader);
            Class<?> entityTypeClass = Class.forName(
                    "net.minecraft.world.entity.EntityType", true, loader);
            Class<?> registryClass = Class.forName(
                    "net.minecraft.core.registries.BuiltInRegistries", true, loader);
            Class<?> registryApiClass = Class.forName(
                    "net.minecraft.core.Registry", true, loader);
            Class<?> resourceLocationClass = Class.forName(
                    "net.minecraft.resources.ResourceLocation", true, loader);
            Class<?> spawnReasonClass = Class.forName(
                    "net.minecraft.world.entity.EntitySpawnReason", true, loader);

            Field entityTypeRegistryField = registryClass.getField("ENTITY_TYPE");

            Method entityIsRemoved = entityClass.getMethod("isRemoved");
            Method resourceLocationParse = resourceLocationClass.getMethod("parse", String.class);
            Method registryGetOptional = registryApiClass.getMethod("getOptional", resourceLocationClass);
            Method entityTypeCreate = entityTypeClass.getMethod("create", levelClass, spawnReasonClass);
            Method clientLevelGetEntity = clientLevelClass.getMethod("getEntity", int.class);
            Method entityGetId = entityClass.getMethod("getId");
            Method entitySetId = entityClass.getMethod("setId", int.class);
            Method entitySetUUID = entityClass.getMethod("setUUID", java.util.UUID.class);
            Method entitySetPos = entityClass.getMethod(
                    "setPos", double.class, double.class, double.class);
            Method entitySetYRot = entityClass.getMethod("setYRot", float.class);
            Method entitySetXRot = entityClass.getMethod("setXRot", float.class);
            Method entitySetYHeadRot = entityClass.getMethod("setYHeadRot", float.class);
            Method entitySetYBodyRot = entityClass.getMethod("setYBodyRot", float.class);
            Method entitySetOldPosAndRot = entityClass.getMethod("setOldPosAndRot");
            Method clientLevelAddEntity = clientLevelClass.getMethod("addEntity", entityClass);
            Method livingSetHealth = livingClass.getMethod("setHealth", float.class);

            NeoForgeEntityBindings result = new NeoForgeEntityBindings(
                    loader,
                    entityClass,
                    livingClass,
                    playerClass,
                    levelClass,
                    clientLevelClass,
                    entityTypeClass,
                    registryClass,
                    registryApiClass,
                    resourceLocationClass,
                    spawnReasonClass,
                    entityTypeRegistryField,
                    entityIsRemoved,
                    resourceLocationParse,
                    registryGetOptional,
                    entityTypeCreate,
                    clientLevelGetEntity,
                    entityGetId,
                    entitySetId,
                    entitySetUUID,
                    entitySetPos,
                    entitySetYRot,
                    entitySetXRot,
                    entitySetYHeadRot,
                    entitySetYBodyRot,
                    entitySetOldPosAndRot,
                    clientLevelAddEntity,
                    livingSetHealth);

            nfEntityBindings = result;
            return result;
        }
    }

    static volatile Object remotePlayer = null;
    static volatile Object remotePlayerLevel = null;
    static final java.util.concurrent.ConcurrentHashMap<java.util.UUID, Object> remoteEntities = new java.util.concurrent.ConcurrentHashMap<>();
    static final MonsterSequenceTracker remoteEntitySequences = new MonsterSequenceTracker(16384);
    static volatile Object remoteEntityLevel = null;
    static volatile int nextRemoteEntityId = 1_800_000_000;
    static final java.util.concurrent.atomic.AtomicBoolean entityErrorPrinted = new java.util.concurrent.atomic.AtomicBoolean(
            false);
    // NeoForge mirror lifecycle state.
    static volatile boolean dataBusConnected = false;
    static volatile boolean nfReconnectCleanupPending = false;
    static volatile long dataBusGeneration = 0L;
    static volatile long lastFabricReceiveAt = 0L;
    static final long FABRIC_TIMEOUT_MS = 3000L;
    static final java.util.concurrent.atomic.AtomicBoolean nfRemovalPending = new java.util.concurrent.atomic.AtomicBoolean(
            false);

    // Monster cleanup is deliberately independent of player cleanup.
    static volatile boolean nfEntityReconnectCleanupPending = false;
    static final MonsterCleanupCoordinator entityCleanupCoordinator = new MonsterCleanupCoordinator();

    static volatile boolean nfInitialized = false;

    static volatile boolean nfMcFoundLogged = false;
    static volatile boolean nfLevelReadyLogged = false;
    static volatile boolean nfPlayerReadyLogged = false;
    static volatile boolean nfProbeErrorPrinted = false;
    static final java.util.concurrent.atomic.AtomicBoolean
            nfEntityBindingProbeAttempted =
                    new java.util.concurrent.atomic.AtomicBoolean(false);

    static final java.util.concurrent.atomic.AtomicBoolean nfProbePending = new java.util.concurrent.atomic.AtomicBoolean(
            false);

    static final java.util.UUID remotePlayerUuid = java.util.UUID.nameUUIDFromBytes(
            "jvm-mod-combiner:fabric-mirror".getBytes(java.nio.charset.StandardCharsets.UTF_8));

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        inst = instrumentation;

        String loaderProperty = System.getProperty("jvmcombiner.loader");
        myName = LoaderDetector.resolve(loaderProperty, agentArgs);

        System.out.println(
                "[BusAgent:" + myName + "] premain" +
                        " | property=" + loaderProperty +
                        " | agentArgs=" + agentArgs);

        if (myName.equals("unknown")) {
            System.err.println(
                    "[BusAgent][ERROR] Unknown Loader identity; " +
                            "DataBus and Loader monitors will not start.");
            return;
        }

        if (loaderProperty != null && !loaderProperty.isBlank()) {
            String legacyName = LoaderDetector.normalize(agentArgs);
            String propertyName = LoaderDetector.normalize(loaderProperty);

            if (!legacyName.equals("unknown") &&
                    !legacyName.equals(propertyName)) {
                System.err.println(
                        "[BusAgent][WARN] Loader identity mismatch: " +
                                "property=" + propertyName +
                                ", agentArgs=" + legacyName +
                                ". System property takes precedence.");
            }
        }

        Thread busThread = new Thread(
                BusAgent::runBus,
                "BusAgent-DataBus");
        busThread.setDaemon(true);
        busThread.start();

        if (myName.equals("fabric")) {
            Thread monitor = new Thread(
                    BusAgent::monitorFabricPlayer,
                    "BusAgent-Fabric-Monitor");
            monitor.setDaemon(true);
            monitor.start();
        } else if (myName.equals("neoforge")) {
            Thread monitor = new Thread(
                    BusAgent::monitorNeoForgePlayer,
                    "BusAgent-NeoForge-Monitor");
            monitor.setDaemon(true);
            monitor.start();
        } else if (myName.equals("forge")) {
            System.out.println(
                    "[BusAgent:forge] Registration-only mode; " +
                            "no Forge player adapter is active.");
        }
    }

    static void runBus() {
        String peer = switch (myName) {
            case "fabric" -> "neoforge";
            case "neoforge" -> "fabric";
            default -> "";
        };

        String socketPath = System.getenv("JVM_COMBINER_BUS_SOCKET");
        if (socketPath == null || socketPath.isBlank()) {
            System.err.println(
                    "[BusAgent:" + myName +
                            "] JVM_COMBINER_BUS_SOCKET is missing");
            return;
        }

        if (peer.isEmpty()) {
            System.err.println(
                    "[BusAgent:" + myName +
                            "] DataBus role is not authorized");
            return;
        }

        while (!Thread.currentThread().isInterrupted()) {
            try (SocketChannel channel = SocketChannel.open(
                    java.net.StandardProtocolFamily.UNIX)) {
                channel.connect(
                        java.net.UnixDomainSocketAddress.of(socketPath));

                try (
                        java.io.InputStream input = Channels.newInputStream(channel);
                        PrintWriter writer = new PrintWriter(
                                new OutputStreamWriter(
                                        Channels.newOutputStream(channel),
                                        java.nio.charset.StandardCharsets.UTF_8),
                                true)) {

                    final long connectionGeneration;

                    synchronized (BusAgent.class) {
                        out = writer;
                        if (myName.equals("neoforge")) {
                            dataBusGeneration++;
                            connectionGeneration = dataBusGeneration;
                            dataBusConnected = true;
                            nfReconnectCleanupPending = true;
                            nfEntityReconnectCleanupPending = true;
                            lastFabricReceiveAt = 0L;
                        } else {
                            connectionGeneration = -1L;
                        }
                    }

                    System.out.println(
                            "[BusAgent:" + myName +
                                    "] Connected to DataBus");

                    if (myName.equals("neoforge")) {
                        scheduleRemotePlayerRemoval(
                                "bus-reconnected",
                                connectionGeneration,
                                2,
                                0L);

                        scheduleRemoteEntityRemoval(
                                "bus-reconnected",
                                connectionGeneration,
                                2);
                    }

                    byte[] bytes;

                    while ((bytes = readBoundedLine(input)) != null) {
                        final java.util.Map<String, Object> message;
                        final String incomingType;

                        try {
                            message = StrictJson.parseUtf8(bytes);
                            if (handleInboundControlMessage(
                                    message, peer, myName)) {
                                continue;
                            }
                            incomingType = validateInboundMessage(
                                    message, peer, myName);
                        } catch (IllegalArgumentException rejected) {
                            logRejectedMessage(rejected.getMessage());
                            continue;
                        }

                        if ("fabric".equals(myName)) {
                            java.util.UUID ackUuid = java.util.UUID.fromString(
                                    StrictJson.string(message, "uuid"));
                            long ackSeq = StrictJson.positiveLong(message, "seq");

                            MonsterSync.acknowledge(ackUuid, ackSeq);
                            continue;
                        }

                        if ("player_move".equals(incomingType)) {
                            synchronized (BusAgent.class) {
                                lastFabricReceiveAt = System.currentTimeMillis();
                            }
                            handleNeoForgeSync(message);
                        } else if ("entity_spawn".equals(incomingType) ||
                                "entity_update".equals(incomingType)) {
                            handleNeoForgeEntitySync(message);
                        } else if ("entity_remove".equals(incomingType)) {
                            handleNeoForgeEntityRemove(message);
                        }
                    }
                } finally {
                    synchronized (BusAgent.class) {
                        out = null;
                    }
                    markDataBusDisconnected("stream-ended");
                }

            } catch (Exception error) {
                synchronized (BusAgent.class) {
                    out = null;
                }
                markDataBusDisconnected("connection-error");

                System.out.println(
                        "[BusAgent:" + myName +
                                "] Bus disconnected: " + error.getMessage());

                try {
                    Thread.sleep(2000);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }
    }

    static synchronized void bindFabricLoaderMethods()
            throws ReflectiveOperationException {
        if (fabricLoaderClass != null &&
                fabricGetInstanceMethod != null &&
                fabricGetGameInstanceMethod != null) {
            return;
        }

        // Build locally and publish only after every lookup succeeds.
        Class<?> candidateClass = Class.forName(
                "net.fabricmc.loader.api.FabricLoader");

        Method getInstance = candidateClass.getMethod("getInstance");

        Method getGameInstance = candidateClass.getMethod("getGameInstance");

        fabricLoaderClass = candidateClass;
        fabricGetInstanceMethod = getInstance;
        fabricGetGameInstanceMethod = getGameInstance;
    }

    static void monitorFabricPlayer() {
        System.out.println("[BusAgent:fabric] monitor thread started");

        while (!Thread.currentThread().isInterrupted()) {
            try {
                if (fabricMcInstance == null) {
                    bindFabricLoaderMethods();

                    Object loader = fabricGetInstanceMethod.invoke(null);

                    Object instance = fabricGetGameInstanceMethod.invoke(loader);

                    if (instance == null) {
                        Thread.sleep(250);
                        continue;
                    }

                    // Fabric runtime uses Intermediary field names.
                    // MinecraftClient.field_1724 is the local player.
                    Field playerField = instance.getClass().getField("field_1724");

                    // execute(Runnable) is inherited from ThreadExecutor.
                    Method executeMethod = instance.getClass().getMethod(
                            "execute",
                            Runnable.class);

                    // Publish the instance only after all lookups succeed.
                    fabricPlayerField = playerField;
                    fabricExecuteMethod = executeMethod;
                    fabricMcInstance = instance;

                    System.out.println(
                            "[BusAgent:fabric][INIT] SUCCESS: " +
                                    instance.getClass().getName());

                    System.out.println(
                            "[BusAgent:fabric][INIT] player field=" +
                                    fabricPlayerField.getName() +
                                    ", executor=" +
                                    fabricExecuteMethod.getDeclaringClass().getName());
                }

                if (fabricSamplePending.compareAndSet(false, true)) {
                    try {
                        fabricExecuteMethod.invoke(
                                fabricMcInstance,
                                (Runnable) () -> {
                                    try {
                                        sampleFabricPlayer();
                                    } catch (Throwable e) {
                                        if (!fabricSampleErrorPrinted) {
                                            fabricSampleErrorPrinted = true;
                                            System.err.println(
                                                    "[BusAgent:fabric][ERROR] player sample failed:");
                                            e.printStackTrace(System.err);
                                        }
                                    }

                                    // M1: sample hostile mobs independently of player sampling.
                                    try {
                                        MonsterSync.sample(fabricMcInstance);
                                    } catch (Throwable mobError) {
                                        System.err.println(
                                                "[BusAgent:fabric][ERROR] MonsterSync invocation failed: " +
                                                        mobError);
                                    } finally {
                                        fabricSamplePending.set(false);
                                    }
                                });
                    } catch (Throwable e) {
                        fabricSamplePending.set(false);
                        throw e;
                    }
                }

                Thread.sleep(50);

            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;

            } catch (Throwable e) {
                fabricSamplePending.set(false);

                System.err.println(
                        "[BusAgent:fabric][ERROR] monitor failed: " + e);

                try {
                    Thread.sleep(1000);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }
    }

    static void sampleFabricPlayer() throws Exception {
        Object mc = fabricMcInstance;
        if (mc == null || fabricPlayerField == null) {
            return;
        }

        Object player = fabricPlayerField.get(mc);
        if (player == null) {
            return;
        }

        if (fabricBindingState == FabricBindingState.UNINITIALIZED) {
            if (!bindMethods(player.getClass())) {
                return;
            }
        }

        if (fabricBindingState != FabricBindingState.READY) {
            return;
        }

        double x = ((Number) getXMethod.invoke(player)).doubleValue();
        double y = ((Number) getYMethod.invoke(player)).doubleValue();
        double z = ((Number) getZMethod.invoke(player)).doubleValue();

        float yaw = ((Number) getYRotMethod.invoke(player)).floatValue();
        float pitch = ((Number) getXRotMethod.invoke(player)).floatValue();

        boolean sprint = (Boolean) isSprintingMethod.invoke(player);

        String state = Double.toString(x) + "|" +
                Double.toString(y) + "|" +
                Double.toString(z) + "|" +
                Float.toString(yaw) + "|" +
                Float.toString(pitch) + "|" +
                sprint;

        long now = System.currentTimeMillis();

        // Send on state changes, or once per second as a heartbeat.
        boolean changed = !state.equals(lastFabricState);
        boolean heartbeat = now - lastFabricSendAt >= 1000L;

        if (changed || heartbeat) {
            sendFabricPlayer(x, y, z, yaw, pitch, sprint, state, now);
        }
    }

    static boolean sendFabricMonster(String json) {
        if (!"fabric".equals(myName) ||
                json == null ||
                json.isEmpty()) {
            return false;
        }

        synchronized (BusAgent.class) {
            PrintWriter writer = out;

            if (writer == null) {
                return false;
            }

            writer.println(json);

            if (writer.checkError()) {
                System.err.println(
                        "[BusAgent:fabric][ENTITY][SEND][ERROR] " +
                                "DataBus write failed");
                return false;
            }
        }

        BusAgent.logDebug(
                "[BusAgent:fabric][ENTITY][SEND] message written");

        return true;
    }

    static boolean sendNeoForgeEntityAck(
            java.util.UUID uuid,
            long seq) {
        if (!"neoforge".equals(myName) ||
                uuid == null ||
                seq <= 0L) {
            return false;
        }

        String json = "{\"to\":\"fabric\"," +
                "\"type\":\"entity_ack\"," +
                "\"uuid\":\"" + uuid + "\"," +
                "\"seq\":" + seq +
                "}";

        synchronized (BusAgent.class) {
            PrintWriter writer = out;

            if (writer == null || !dataBusConnected) {
                return false;
            }

            writer.println(json);

            if (writer.checkError()) {
                System.err.println(
                        "[BusAgent:neoforge][ENTITY][ACK][ERROR] " +
                                "DataBus write failed");
                return false;
            }
        }

        BusAgent.logDebug(
                "[BusAgent:neoforge][ENTITY][ACK] uuid=" +
                        uuid + " seq=" + seq);

        return true;
    }

    static void sendFabricPlayer(
            double x,
            double y,
            double z,
            float yaw,
            float pitch,
            boolean sprint,
            String state,
            long now) {
        PrintWriter writer = out;

        if (writer == null) {
            return;
        }

        String json = "{\"to\":\"neoforge\",\"type\":\"player_move\"," +
                "\"x\":" + Double.toString(x) +
                ",\"y\":" + Double.toString(y) +
                ",\"z\":" + Double.toString(z) +
                ",\"yaw\":" + Float.toString(yaw) +
                ",\"pitch\":" + Float.toString(pitch) +
                ",\"sprint\":" + sprint +
                "}";

        synchronized (BusAgent.class) {
            // Recheck the writer after acquiring the lock.
            writer = out;

            if (writer == null) {
                return;
            }

            writer.println(json);

            if (writer.checkError()) {
                System.err.println(
                        "[BusAgent:fabric][SEND][ERROR] DataBus write failed");
                return;
            }
        }

        lastFabricState = state;
        lastFabricSendAt = now;

        BusAgent.logDebug(
                "[BusAgent:fabric][SEND] x=" + x +
                        " y=" + y +
                        " z=" + z +
                        " yaw=" + yaw +
                        " pitch=" + pitch +
                        " sprint=" + sprint);
    }

    private static Object enumConstant(
            Class<?> enumClass, String constantName) {
        if (enumClass == null || !enumClass.isEnum()) {
            throw new IllegalArgumentException(
                    "Expected enum class: " + enumClass);
        }
        for (Object constant : enumClass.getEnumConstants()) {
            if (((Enum<?>) constant).name().equals(constantName)) {
                return constant;
            }
        }
        throw new IllegalArgumentException(
                "Missing enum constant " + enumClass.getName() +
                        "." + constantName);
    }

    static void monitorNeoForgePlayer() {
        System.out.println(
                "[BusAgent:neoforge][MONITOR] thread started");

        while (!Thread.currentThread().isInterrupted()) {
            try {
                initNeoForgeMinecraft();

                checkRemotePlayerLifecycle();
                checkRemoteEntityLifecycle();

                Object mc = nfMcInstance;

                if (mc != null) {
                    if (!nfMcFoundLogged) {
                        nfMcFoundLogged = true;
                        System.out.println(
                                "[BusAgent:neoforge][INIT] Minecraft instance found");
                    }

                    if (nfProbePending.compareAndSet(false, true)) {
                        try {
                            nfExecuteMethod.invoke(mc, (Runnable) () -> {
                                try {
                                    Object level = nfLevelField.get(mc);

                                    Object player = nfPlayerField.get(mc);

                                    boolean levelReady = level != null;
                                    boolean playerReady = levelReady && player != null;

                                    if (levelReady &&
                                            nfEntityBindingProbeAttempted.compareAndSet(false, true)) {
                                        try {
                                            NeoForgeEntityBindings bindings =
                                                    getNeoForgeEntityBindings(nfMcClass.getClassLoader());
                                            System.out.println(
                                                    "[BusAgent:neoforge][ENTITY][BIND][OK] " +
                                                    "Entity=" + bindings.entityClass.getName() +
                                                    ", LivingEntity=" + bindings.livingClass.getName());
                                        } catch (Throwable bindingError) {
                                            System.err.println(
                                                    "[BusAgent:neoforge][ENTITY][BIND][ERROR] " +
                                                    bindingError);
                                            bindingError.printStackTrace(System.err);
                                        }
                                    }

                                    synchronized (BusAgent.class) {
                                        if (levelReady != nfLevelReadyLogged) {
                                            nfLevelReadyLogged = levelReady;

                                            System.out.println(
                                                    levelReady
                                                            ? "[BusAgent:neoforge][INIT] ClientLevel ready"
                                                            : "[BusAgent:neoforge][WAIT] ClientLevel unavailable");
                                        }

                                        if (playerReady != nfPlayerReadyLogged) {
                                            nfPlayerReadyLogged = playerReady;

                                            System.out.println(
                                                    playerReady
                                                            ? "[BusAgent:neoforge][LIVE] LocalPlayer ready"
                                                            : "[BusAgent:neoforge][WAIT] LocalPlayer unavailable");
                                        }
                                    }

                                    // NeoForge is a receiver-only client.
                                    // Fabric is the sole authoritative player-state source.
                                    // Never sample or transmit NeoForge LocalPlayer state.

                                } catch (Throwable e) {
                                    if (!nfProbeErrorPrinted) {
                                        nfProbeErrorPrinted = true;
                                        System.err.println(
                                                "[BusAgent:neoforge][ERROR] player probe failed: " + e);
                                        e.printStackTrace(System.err);
                                    }
                                } finally {
                                    nfProbePending.set(false);
                                }
                            });
                        } catch (Throwable e) {
                            nfProbePending.set(false);

                            if (!nfProbeErrorPrinted) {
                                nfProbeErrorPrinted = true;
                                System.err.println(
                                        "[BusAgent:neoforge][ERROR] execute failed: " + e);
                            }
                        }
                    }
                }

                Thread.sleep(250);

            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;

            } catch (Throwable e) {
                System.err.println(
                        "[BusAgent:neoforge][ERROR] monitor failed: " + e);

                try {
                    Thread.sleep(1000);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }
    }

    static synchronized void initNeoForgeMinecraft() {
        if (nfInitialized && nfMcInstance != null) {
            return;
        }

        try {
            Class<?> candidateClass = nfMcClass;

            if (candidateClass == null) {
                for (Class<?> c : inst.getAllLoadedClasses()) {
                    if (c.getName().equals("net.minecraft.client.Minecraft")) {
                        candidateClass = c;
                        break;
                    }
                }
            }

            if (candidateClass == null) {
                return;
            }

            // Bind everything locally before publishing initialized state.
            Method getInstanceMethod = candidateClass.getMethod("getInstance");
            Method executeMethod = candidateClass.getMethod("execute", Runnable.class);
            Field levelField = candidateClass.getField("level");
            Field playerField = candidateClass.getField("player");

            Object minecraft = getInstanceMethod.invoke(null);
            if (minecraft == null) {
                return;
            }

            // Publish the complete binding before exposing the instance.
            nfGetInstanceMethod = getInstanceMethod;
            nfExecuteMethod = executeMethod;
            nfLevelField = levelField;
            nfPlayerField = playerField;
            nfMcClass = candidateClass;
            nfMcInstance = minecraft;
            nfInitialized = true;

            System.out.println("[BusAgent:neoforge] ✅ 成功锁定 Minecraft 实例！");
        } catch (Throwable e) {
            nfInitialized = false;
            nfMcInstance = null;
            nfGetInstanceMethod = null;
            nfExecuteMethod = null;
            nfLevelField = null;
            nfPlayerField = null;

            System.out.println("[BusAgent:neoforge] init failed: " + e);
        }
    }

    static void handleNeoForgeSync(java.util.Map<String, Object> message) {
        try {
            if (!nfInitialized || nfMcInstance == null) {
                initNeoForgeMinecraft();
                if (nfMcInstance == null)
                    return;
            }

            double x = StrictJson.finiteDouble(message, "x");
            double y = StrictJson.finiteDouble(message, "y");
            double z = StrictJson.finiteDouble(message, "z");
            float yaw = StrictJson.finiteFloat(message, "yaw");
            float pitch = StrictJson.finiteFloat(message, "pitch");
            boolean sprint = StrictJson.bool(message, "sprint");

            final long syncGeneration;
            synchronized (BusAgent.class) {
                if (!dataBusConnected) {
                    return;
                }
                syncGeneration = dataBusGeneration;
            }

            Object mc = nfMcInstance;
            nfExecuteMethod.invoke(mc, (Runnable) () -> {
                try {
                    synchronized (BusAgent.class) {
                        if (!dataBusConnected ||
                                dataBusGeneration != syncGeneration) {
                            return;
                        }
                    }

                    Object level = nfLevelField.get(mc);
                    if (level == null)
                        return;
                    ClassLoader loader = nfMcClass.getClassLoader();
                    NeoForgePlayerBindings bindings = getNeoForgePlayerBindings(loader);

                    if (remotePlayer != null && remotePlayerLevel != level) {
                        removeRemotePlayerNow("world-changed");
                    }

                    if (remotePlayer == null) {
                        Object profile = bindings.profileConstructor
                                .newInstance(remotePlayerUuid, "FabricMirror");
                        Object player = bindings.playerConstructor
                                .newInstance(level, profile);

                        Method getEntity = bindings.getEntity;
                        Method setId = bindings.setId;
                        Method setUUID = bindings.setUUID;

                        int candidate = 1_900_000_000;
                        boolean foundFreeId = false;

                        for (int i = 0; i < 10000; i++) {
                            if (getEntity.invoke(level, candidate) == null) {
                                foundFreeId = true;
                                break;
                            }

                            candidate--;
                        }

                        if (!foundFreeId) {
                            throw new IllegalStateException(
                                    "Unable to allocate remote player entity ID");
                        }

                        setId.invoke(player, candidate);
                        setUUID.invoke(player, remotePlayerUuid);

                        bindings.setPos.invoke(player, x, y, z);
                        bindings.setOldPosAndRot.invoke(player);

                        bindings.addEntity.invoke(level, player);
                        remotePlayer = player;
                        remotePlayerLevel = level;

                        System.out.println("[BusAgent:neoforge] ✅ 成功创建镜像实体！ID=" + candidate);
                    } else {
                        bindings.setPos.invoke(remotePlayer, x, y, z);
                        bindings.setOldPosAndRot.invoke(remotePlayer);
                    }

                    try {
                        if (bindings.setYRot != null) {
                            bindings.setYRot.invoke(remotePlayer, yaw);
                        }
                    } catch (Throwable ignored) {
                    }
                    try {
                        if (bindings.setXRot != null) {
                            bindings.setXRot.invoke(remotePlayer, pitch);
                        }
                    } catch (Throwable ignored) {
                    }
                    try {
                        if (bindings.setYHeadRot != null) {
                            bindings.setYHeadRot.invoke(remotePlayer, yaw);
                        }
                    } catch (Throwable ignored) {
                    }
                    try {
                        if (bindings.setYBodyRot != null) {
                            bindings.setYBodyRot.invoke(remotePlayer, yaw);
                        }
                    } catch (Throwable ignored) {
                    }
                    try {
                        if (bindings.setSprinting != null) {
                            bindings.setSprinting.invoke(remotePlayer, sprint);
                        }
                    } catch (Throwable ignored) {
                    }
                    BusAgent.logDebug(
                            "[BusAgent:neoforge] 同步 -> (" + x + "," + y + "," + z + ") yaw=" + yaw + " pitch=" + pitch);

                } catch (Throwable e) {
                    System.out.println("[BusAgent:neoforge] Runnable err: " + e);
                }
            });
        } catch (Throwable e) {
            System.out.println("[BusAgent:neoforge] handle err: " + e);
        }
    }

    private static String entitySyncFingerprint(
            String operationType,
            String entityTypeId,
            double x,
            double y,
            double z,
            float yaw,
            float pitch,
            float health,
            float maxHealth) {
        return "sync\u0000" + operationType +
                "\u0000" + entityTypeId +
                "\u0000" + Double.toHexString(x) +
                "\u0000" + Double.toHexString(y) +
                "\u0000" + Double.toHexString(z) +
                "\u0000" + Float.toHexString(yaw) +
                "\u0000" + Float.toHexString(pitch) +
                "\u0000" + Float.toHexString(health) +
                "\u0000" + Float.toHexString(maxHealth);
    }

    static void handleNeoForgeEntitySync(java.util.Map<String, Object> message) {
        try {
            if (!nfInitialized || nfMcInstance == null) {
                initNeoForgeMinecraft();

                if (nfMcInstance == null) {
                    return;
                }
            }

            java.util.UUID uuid = java.util.UUID.fromString(
                    StrictJson.string(message, "uuid"));

            long seq = StrictJson.positiveLong(message, "seq");

            String operationType = StrictJson.string(message, "type");

            String entityTypeId = StrictJson.string(message, "entity_type");

            double x = StrictJson.finiteDouble(message, "x");
            double y = StrictJson.finiteDouble(message, "y");
            double z = StrictJson.finiteDouble(message, "z");

            float yaw = StrictJson.finiteFloat(message, "yaw");
            float pitch = StrictJson.finiteFloat(message, "pitch");

            float health = StrictJson.finiteFloat(message, "health");
            float maxHealth = StrictJson.finiteFloat(message, "max_health");

            if (!Double.isFinite(x) ||
                    !Double.isFinite(y) ||
                    !Double.isFinite(z) ||
                    !Float.isFinite(yaw) ||
                    !Float.isFinite(pitch) ||
                    !Float.isFinite(health) ||
                    !Float.isFinite(maxHealth) ||
                    maxHealth < 0.0f) {

                throw new IllegalArgumentException(
                        "Invalid monster numeric state: " + uuid);
            }

            final String fingerprint = entitySyncFingerprint(
                    operationType, entityTypeId,
                    x, y, z, yaw, pitch, health, maxHealth);

            final long generation;

            synchronized (BusAgent.class) {
                if (!dataBusConnected || nfEntityReconnectCleanupPending) {
                    return;
                }

                generation = dataBusGeneration;
            }

            Object mc = nfMcInstance;

            nfExecuteMethod.invoke(mc, (Runnable) () -> {
                try {
                    synchronized (BusAgent.class) {
                        if (!dataBusConnected ||
                                dataBusGeneration != generation ||
                                nfEntityReconnectCleanupPending) {
                            return;
                        }

                        MonsterSequenceTracker.Decision decision = remoteEntitySequences.inspect(
                                uuid, seq, fingerprint);
                        if (decision == MonsterSequenceTracker.Decision.DUPLICATE) {
                            sendNeoForgeEntityAck(uuid, seq);
                            return;
                        }
                        if (decision != MonsterSequenceTracker.Decision.ACCEPT) {
                            logDebug(
                                    "[BusAgent:neoforge][ENTITY][REJECT] " +
                                            "Sequence rejected: " + decision +
                                            " uuid=" + uuid + " seq=" + seq);
                            return;
                        }
                    }

                    Object level = nfLevelField.get(mc);

                    if (level == null) {
                        return;
                    }

                    ClassLoader loader = nfMcClass.getClassLoader();
                    NeoForgeEntityBindings bindings = getNeoForgeEntityBindings(loader);

                    Class<?> entityClass = bindings.entityClass;
                    Class<?> livingClass = bindings.livingClass;
                    Class<?> levelClass = bindings.levelClass;
                    Class<?> clientLevelClass = bindings.clientLevelClass;
                    Class<?> entityTypeClass = bindings.entityTypeClass;
                    Class<?> registryClass = bindings.registryClass;
                    Class<?> registryApiClass = bindings.registryApiClass;
                    Class<?> resourceLocationClass = bindings.resourceLocationClass;
                    Class<?> spawnReasonClass = bindings.spawnReasonClass;

                    // A world transition must not silently forget
                    // entities that could not be removed.
                    if (remoteEntityLevel != null &&
                            remoteEntityLevel != level) {
                        if (!removeRemoteEntitiesNow("world-changed") ||
                                !remoteEntities.isEmpty()) {
                            return;
                        }
                    }

                    remoteEntityLevel = level;

                    Object entity = remoteEntities.get(uuid);

                    if (entity != null) {
                        boolean removed = Boolean.TRUE.equals(
                                bindings.entityIsRemoved
                                        .invoke(entity));

                        if (removed) {
                            remoteEntities.remove(uuid, entity);
                            entity = null;
                        }
                    }

                    if (entity == null) {
                        if (remoteEntities.size() >= 1024) {
                            logDebug(
                                    "[BusAgent:neoforge][ENTITY][LIMIT] " +
                                            "Remote monster capacity reached; uuid=" +
                                            uuid);
                            return;
                        }

                        Object resourceLocation = bindings.resourceLocationParse
                                .invoke(null, entityTypeId);

                        Object registry = bindings.entityTypeRegistryField
                                .get(null);

                        Object optional = bindings.registryGetOptional
                                .invoke(registry, resourceLocation);

                        if (!(optional instanceof java.util.Optional<?>)) {
                            throw new IllegalStateException(
                                    "Entity registry lookup returned invalid result");
                        }

                        java.util.Optional<?> found = (java.util.Optional<?>) optional;

                        if (found.isEmpty()) {
                            System.err.println(
                                    "[BusAgent:neoforge][ENTITY][SKIP] " +
                                            "Unknown entity type: " + entityTypeId);
                            return;
                        }

                        Object entityType = found.get();

                        Object spawnReason = enumConstant(
                                spawnReasonClass,
                                "COMMAND");

                        entity = bindings.entityTypeCreate
                                .invoke(entityType, level, spawnReason);

                        if (entity == null) {
                            System.err.println(
                                    "[BusAgent:neoforge][ENTITY][SKIP] " +
                                            "Entity creation returned null: " +
                                            entityTypeId);
                            return;
                        }

                        if (!livingClass.isInstance(entity) ||
                                bindings.playerClass.isInstance(entity)) {
                            System.err.println(
                                    "[BusAgent:neoforge][ENTITY][REJECT] " +
                                            "Mirror must be a non-player LivingEntity: " +
                                            entityTypeId);
                            return;
                        }

                        int candidate = nextRemoteEntityId;

                        Method getEntity = bindings.clientLevelGetEntity;

                        Method setId = bindings.entitySetId;

                        boolean foundFreeId = false;

                        for (int i = 0; i < 10000; i++) {
                            if (getEntity.invoke(level, candidate) == null) {
                                foundFreeId = true;
                                break;
                            }

                            candidate--;
                        }

                        if (!foundFreeId) {
                            throw new IllegalStateException(
                                    "Unable to allocate remote monster entity ID");
                        }

                        nextRemoteEntityId = candidate - 1;

                        setId.invoke(entity, candidate);

                        bindings.entitySetUUID.invoke(entity, uuid);

                        bindings.entitySetPos.invoke(entity, x, y, z);

                        bindings.entitySetOldPosAndRot.invoke(entity);

                        remoteEntities.put(uuid, entity);

                        System.out.println(
                                "[BusAgent:neoforge][ENTITY][SPAWN] " +
                                        "type=" + entityTypeId +
                                        " uuid=" + uuid +
                                        " id=" + candidate);
                    }

                    // Verify that this exact entity instance is registered
                    // in the current ClientLevel before applying state or ACKing.
                    int entityId = ((Number) bindings.entityGetId
                            .invoke(entity)).intValue();

                    Object registeredEntity = bindings.clientLevelGetEntity
                            .invoke(level, entityId);

                    if (registeredEntity != entity) {
                        if (registeredEntity != null) {
                            throw new IllegalStateException(
                                    "Remote monster entity ID is occupied by another entity: " +
                                            entityId + " uuid=" + uuid);
                        }

                        // A previous addEntity attempt may have failed.
                        // Keep the map reference and retry registration.
                        bindings.clientLevelAddEntity.invoke(level, entity);

                        registeredEntity = bindings.clientLevelGetEntity
                                .invoke(level, entityId);

                        if (registeredEntity != entity) {
                            throw new IllegalStateException(
                                    "Remote monster registration could not be verified: " +
                                            uuid);
                        }
                    }

                    bindings.entitySetPos.invoke(entity, x, y, z);

                    bindings.entitySetYRot.invoke(entity, yaw);

                    bindings.entitySetXRot.invoke(entity, pitch);

                    bindings.entitySetYHeadRot.invoke(entity, yaw);

                    bindings.entitySetYBodyRot.invoke(entity, yaw);

                    bindings.entitySetOldPosAndRot.invoke(entity);

                    if (livingClass.isInstance(entity)) {
                        bindings.livingSetHealth.invoke(
                                entity,
                                Math.max(0.0f, Math.min(health, maxHealth)));
                    }

                    // ACK only if this task still belongs to the
                    // active DataBus generation.
                    synchronized (BusAgent.class) {
                        if (!dataBusConnected ||
                                dataBusGeneration != generation ||
                                nfEntityReconnectCleanupPending) {
                            return;
                        }

                        remoteEntitySequences.commit(
                                uuid, seq, fingerprint);
                        sendNeoForgeEntityAck(uuid, seq);
                    }

                } catch (Throwable error) {
                    if (entityErrorPrinted.compareAndSet(false, true)) {
                        System.err.println(
                                "[BusAgent:neoforge][ENTITY][ERROR] " +
                                        "Monster main-thread handler failed");

                        error.printStackTrace(System.err);
                    }
                }
            });

        } catch (Throwable error) {
            if (entityErrorPrinted.compareAndSet(false, true)) {
                System.err.println(
                        "[BusAgent:neoforge][ENTITY][ERROR] " +
                                "Monster message rejected");

                error.printStackTrace(System.err);
            }
        }
    }

    // Must only be called on the NeoForge Minecraft game thread.
    // Entries are removed from the map only after entity removal is verified.
    static boolean removeRemoteEntitiesNow(String reason) {
        NeoForgeEntityBindings bindings;

        try {
            Object mc = nfMcInstance;
            if (mc == null || nfMcClass == null) {
                return remoteEntities.isEmpty();
            }

            bindings = getNeoForgeEntityBindings(
                    nfMcClass.getClassLoader());

        } catch (Throwable error) {
            System.err.println(
                    "[BusAgent:neoforge][ENTITY][CLEANUP][ERROR] " +
                            "Could not resolve removal bindings: " + error);
            return remoteEntities.isEmpty();
        }

        boolean allRemoved = true;

        for (java.util.Map.Entry<java.util.UUID, Object> entry : remoteEntities.entrySet()) {
            java.util.UUID uuid = entry.getKey();
            Object entity = entry.getValue();

            try {
                if (!Boolean.TRUE.equals(
                        bindings.entityIsRemoved.invoke(entity))) {
                    entity.getClass()
                            .getMethod("discard")
                            .invoke(entity);
                }

                boolean removed = Boolean.TRUE.equals(
                        bindings.entityIsRemoved.invoke(entity));

                if (removed) {
                    remoteEntities.remove(uuid, entity);
                } else {
                    allRemoved = false;
                    System.err.println(
                            "[BusAgent:neoforge][ENTITY][CLEANUP] " +
                                    "Entity remains; retaining reference uuid=" + uuid);
                }

            } catch (Throwable error) {
                allRemoved = false;
                System.err.println(
                        "[BusAgent:neoforge][ENTITY][CLEANUP][ERROR] " +
                                "Removal failed; retaining uuid=" + uuid +
                                " error=" + error);
            }
        }

        if (remoteEntities.isEmpty()) {
            remoteEntityLevel = null;
            System.out.println(
                    "[BusAgent:neoforge][ENTITY][LIFECYCLE] " +
                            "All remote monsters removed reason=" + reason);
            return true;
        }

        return allRemoved && remoteEntities.isEmpty();
    }

    // Called by the NeoForge DataBus monitor thread.
    static void checkRemoteEntityLifecycle() {
        if (!"neoforge".equals(myName)) {
            return;
        }

        final boolean connected;
        final boolean reconnectCleanup;
        final long generation;

        synchronized (BusAgent.class) {
            connected = dataBusConnected;
            reconnectCleanup = nfEntityReconnectCleanupPending;
            generation = dataBusGeneration;
        }

        if (!connected || reconnectCleanup) {
            scheduleRemoteEntityRemoval(
                    connected ? "bus-reconnected" : "bus-disconnected",
                    generation,
                    connected ? 2 : 0);
        }
    }

    // condition 0: cleanup only while disconnected.
    // condition 2: cleanup only for the current connected generation.
    static void scheduleRemoteEntityRemoval(
            String reason,
            long generation,
            int condition) {

        Object mc = nfMcInstance;

        if (mc == null || nfMcClass == null) {
            return;
        }

        final Object cleanupLease = entityCleanupCoordinator.tryAcquire();

        if (cleanupLease == null) {
            return;
        }

        try {
            nfExecuteMethod.invoke(
                    mc,
                    (Runnable) () -> {
                        try {
                            synchronized (BusAgent.class) {
                                if (!entityCleanupCoordinator.isEligible(
                                        dataBusGeneration,
                                        generation,
                                        dataBusConnected,
                                        condition)) {
                                    return;
                                }
                            }

                            boolean removed = removeRemoteEntitiesNow(reason);

                            if (removed && condition == 2) {
                                synchronized (BusAgent.class) {
                                    if (entityCleanupCoordinator.canReleaseBarrier(
                                            removed,
                                            condition,
                                            dataBusGeneration,
                                            generation,
                                            dataBusConnected,
                                            remoteEntities.isEmpty(),
                                            nfEntityReconnectCleanupPending)) {
                                        remoteEntitySequences.clear();
                                        nfEntityReconnectCleanupPending = false;
                                    }
                                }
                            }

                        } catch (Throwable error) {
                            System.err.println(
                                    "[BusAgent:neoforge][ENTITY][CLEANUP][ERROR] " +
                                            "Scheduled cleanup failed: " + error);
                        } finally {
                            entityCleanupCoordinator.release(cleanupLease);
                        }
                    });

        } catch (Throwable error) {
            entityCleanupCoordinator.release(cleanupLease);

            System.err.println(
                    "[BusAgent:neoforge][ENTITY][CLEANUP][ERROR] " +
                            "Could not schedule cleanup: " + error);
        }
    }

    static void handleNeoForgeEntityRemove(
            java.util.Map<String, Object> message) {
        try {
            if (!nfInitialized || nfMcInstance == null) {
                initNeoForgeMinecraft();
                if (nfMcInstance == null) {
                    return;
                }
            }

            final java.util.UUID uuid = java.util.UUID.fromString(
                    StrictJson.string(message, "uuid"));
            final long seq = StrictJson.positiveLong(message, "seq");
            final String fingerprint = "remove";

            final long generation;
            synchronized (BusAgent.class) {
                if (!dataBusConnected || nfEntityReconnectCleanupPending) {
                    return;
                }
                generation = dataBusGeneration;
            }

            Object mc = nfMcInstance;

            nfExecuteMethod.invoke(mc, (Runnable) () -> {
                try {
                    synchronized (BusAgent.class) {
                        if (!dataBusConnected ||
                                dataBusGeneration != generation ||
                                nfEntityReconnectCleanupPending) {
                            return;
                        }

                        MonsterSequenceTracker.Decision decision = remoteEntitySequences.inspect(
                                uuid, seq, fingerprint);
                        if (decision == MonsterSequenceTracker.Decision.DUPLICATE) {
                            sendNeoForgeEntityAck(uuid, seq);
                            return;
                        }
                        if (decision != MonsterSequenceTracker.Decision.ACCEPT) {
                            logDebug(
                                    "[BusAgent:neoforge][ENTITY][REMOVE][REJECT] " +
                                            "Sequence rejected: " + decision +
                                            " uuid=" + uuid + " seq=" + seq);
                            return;
                        }
                    }

                    Object level = nfLevelField.get(mc);
                    if (level == null) {
                        return;
                    }

                    Object entity = remoteEntities.get(uuid);

                    if (entity != null) {
                        NeoForgeEntityBindings bindings = getNeoForgeEntityBindings(
                                nfMcClass.getClassLoader());

                        if (!Boolean.TRUE.equals(
                                bindings.entityIsRemoved.invoke(entity))) {
                            entity.getClass()
                                    .getMethod("discard")
                                    .invoke(entity);
                        }

                        if (!Boolean.TRUE.equals(
                                bindings.entityIsRemoved.invoke(entity))) {
                            throw new IllegalStateException(
                                    "Monster removal not confirmed: " + uuid);
                        }

                        remoteEntities.remove(uuid, entity);
                    }

                    if (remoteEntities.isEmpty()) {
                        remoteEntityLevel = level;
                    }

                    // Keep generation validation and ACK write atomic with
                    // respect to disconnect/reconnect state transitions.
                    synchronized (BusAgent.class) {
                        if (!dataBusConnected ||
                                dataBusGeneration != generation ||
                                nfEntityReconnectCleanupPending) {
                            return;
                        }

                        remoteEntitySequences.commit(
                                uuid, seq, fingerprint);
                        sendNeoForgeEntityAck(uuid, seq);
                    }

                } catch (Throwable error) {
                    if (entityErrorPrinted.compareAndSet(false, true)) {
                        System.err.println(
                                "[BusAgent:neoforge][ENTITY][REMOVE][ERROR] " +
                                        "Removal was not acknowledged");
                        error.printStackTrace(System.err);
                    }
                }
            });

        } catch (Throwable error) {
            if (entityErrorPrinted.compareAndSet(false, true)) {
                System.err.println(
                        "[BusAgent:neoforge][ENTITY][REMOVE][ERROR] " +
                                "Invalid removal message");
                error.printStackTrace(System.err);
            }
        }
    }

    // Must only be called on the NeoForge Minecraft game thread.
    static void removeRemotePlayerNow(String reason) {
        Object player = remotePlayer;

        if (player == null) {
            remotePlayerLevel = null;
            return;
        }

        boolean removed = false;
        Throwable removalError = null;

        try {
            player.getClass().getMethod("discard").invoke(player);
            removed = true;
        } catch (Throwable discardError) {
            removalError = discardError;

            try {
                ClassLoader loader = player.getClass().getClassLoader();

                Class<?> reasonClass = Class.forName(
                        "net.minecraft.world.entity.Entity$RemovalReason",
                        true,
                        loader);

                Object discarded = enumConstant(
                        reasonClass,
                        "DISCARDED");

                player.getClass()
                        .getMethod("remove", reasonClass)
                        .invoke(player, discarded);

                removed = true;

            } catch (Throwable fallbackError) {
                removalError = fallbackError;

                try {
                    removed = Boolean.TRUE.equals(
                            player.getClass()
                                    .getMethod("isRemoved")
                                    .invoke(player));
                } catch (Throwable ignored) {
                    // Keep the reference so a later lifecycle check can retry.
                }
            }
        }

        if (removed) {
            if (remotePlayer == player) {
                remotePlayer = null;
                remotePlayerLevel = null;
            }

            System.out.println(
                    "[BusAgent:neoforge][LIFECYCLE] "
                            + "Mirror removed reason=" + reason);
        } else {
            System.err.println(
                    "[BusAgent:neoforge][LIFECYCLE][ERROR] "
                            + "Mirror removal failed; reference retained. reason="
                            + reason + " error=" + removalError);
        }
    }

    static void markDataBusDisconnected(String reason) {
        if (!"neoforge".equals(myName)) {
            return;
        }

        final long generation;

        synchronized (BusAgent.class) {
            if (!dataBusConnected) {
                return;
            }

            dataBusConnected = false;
            lastFabricReceiveAt = 0L;
            dataBusGeneration++;
            generation = dataBusGeneration;
            out = null;
            nfEntityReconnectCleanupPending = true;
        }

        System.out.println(
                "[BusAgent:neoforge][LIFECYCLE] DataBus disconnected: " + reason);

        scheduleRemotePlayerRemoval(
                "bus-disconnected",
                generation,
                0,
                0L);

        scheduleRemoteEntityRemoval(
                "bus-disconnected",
                generation,
                0);
    }

    // Called by the NeoForge monitor thread.
    static void checkRemotePlayerLifecycle() {
        if (!"neoforge".equals(myName)) {
            return;
        }

        final boolean connected;
        final long generation;
        final long lastReceive;
        final boolean reconnectCleanup;

        synchronized (BusAgent.class) {
            connected = dataBusConnected;
            generation = dataBusGeneration;
            lastReceive = lastFabricReceiveAt;
            reconnectCleanup = nfReconnectCleanupPending;
        }

        if (connected && reconnectCleanup) {
            if (remotePlayer == null) {
                synchronized (BusAgent.class) {
                    if (dataBusGeneration == generation &&
                            dataBusConnected &&
                            remotePlayer == null) {
                        nfReconnectCleanupPending = false;
                    }
                }
                return;
            }

            scheduleRemotePlayerRemoval(
                    "bus-reconnected",
                    generation,
                    2,
                    0L);
            return;
        }

        if (remotePlayer == null) {
            return;
        }

        if (!connected) {
            scheduleRemotePlayerRemoval(
                    "bus-disconnected",
                    generation,
                    0,
                    0L);
            return;
        }

        if (lastReceive > 0L &&
                System.currentTimeMillis() - lastReceive > FABRIC_TIMEOUT_MS) {

            scheduleRemotePlayerRemoval(
                    "fabric-heartbeat-timeout",
                    generation,
                    1,
                    lastReceive);
        }
    }

    // condition:
    // 0 = remove only while disconnected
    // 1 = remove only if the expected heartbeat is still stale
    // 2 = force cleanup for the current connection generation
    static void scheduleRemotePlayerRemoval(
            String reason,
            long generation,
            int condition,
            long expectedReceiveAt) {
        Object mc = nfMcInstance;
        Class<?> mcClass = nfMcClass;

        if (mc == null || mcClass == null || remotePlayer == null) {
            return;
        }

        if (!nfRemovalPending.compareAndSet(false, true)) {
            return;
        }

        try {
            nfExecuteMethod.invoke(
                    mc,
                    (Runnable) () -> {
                        try {
                            synchronized (BusAgent.class) {
                                if (dataBusGeneration != generation) {
                                    return;
                                }

                                if (condition == 0 && dataBusConnected) {
                                    return;
                                }

                                if (condition == 1) {
                                    if (!dataBusConnected ||
                                            lastFabricReceiveAt != expectedReceiveAt ||
                                            expectedReceiveAt == 0L ||
                                            System.currentTimeMillis() - expectedReceiveAt <= FABRIC_TIMEOUT_MS) {
                                        return;
                                    }

                                    // Prevent repeated timeout scheduling.
                                    lastFabricReceiveAt = 0L;
                                }

                                if (condition == 2 && !dataBusConnected) {
                                    return;
                                }
                            }

                            removeRemotePlayerNow(reason);
                            if (condition == 2) {
                                synchronized (BusAgent.class) {
                                    if (dataBusGeneration == generation &&
                                            dataBusConnected) {
                                        nfReconnectCleanupPending = false;
                                    }
                                }
                            }

                        } catch (Throwable e) {
                            System.err.println(
                                    "[BusAgent:neoforge][LIFECYCLE][ERROR] "
                                            + "Scheduled cleanup failed: " + e);
                        } finally {
                            nfRemovalPending.set(false);
                        }
                    });

        } catch (Throwable e) {
            nfRemovalPending.set(false);

            System.err.println(
                    "[BusAgent:neoforge][LIFECYCLE][ERROR] "
                            + "Could not schedule cleanup: " + e);
        }
    }

    static boolean bindMethods(Class<?> clazz) {
        if (fabricBindingState == FabricBindingState.READY) {
            return true;
        }

        long now = System.currentTimeMillis();
        if (now - lastFabricBindAttemptAt < 3000L) {
            return false;
        }
        lastFabricBindAttemptAt = now;

        System.out.println(
                "[BusAgent:fabric] binding Minecraft 1.21.4 methods: " +
                        clazz.getName());

        try {
            // Resolve every method into local variables first.
            // Do not publish a partially initialized binding.
            // Fabric 1.21.4 Intermediary names, declared on Entity.
            Method x = clazz.getMethod("method_23317");
            Method y = clazz.getMethod("method_23318");
            Method z = clazz.getMethod("method_23321");
            Method yaw = clazz.getMethod("method_36454");
            Method pitch = clazz.getMethod("method_36455");
            Method sprint = clazz.getMethod("method_5624");

            // Commit only after all six lookups succeed.
            getXMethod = x;
            getYMethod = y;
            getZMethod = z;
            getYRotMethod = yaw;
            getXRotMethod = pitch;
            isSprintingMethod = sprint;

            fabricBindingState = FabricBindingState.READY;
            fabricSampleErrorPrinted = false;

            System.out.println(
                    "[BusAgent:fabric] binding result X=" + nameOf(x) +
                            " Y=" + nameOf(y) +
                            " Z=" + nameOf(z) +
                            " Yaw=" + nameOf(yaw) +
                            " Pitch=" + nameOf(pitch) +
                            " Sprint=" + nameOf(sprint));

            System.out.println(
                    "[BusAgent:fabric] method binding state: READY");

            return true;

        } catch (NoSuchMethodException | SecurityException e) {
            // Keep the binding retryable; the monitor retries at most
            // once every three seconds.
            fabricBindingState = FabricBindingState.UNINITIALIZED;

            if (!fabricSampleErrorPrinted) {
                System.err.println(
                        "[BusAgent:fabric][ERROR] 1.21.4 method binding failed; " +
                                "will retry in 3 seconds");
                e.printStackTrace(System.err);
                fabricSampleErrorPrinted = true;
            }

            return false;
        }
    }

    static String nameOf(Method m) {
        return m == null ? "null" : m.getDeclaringClass().getSimpleName() + "." + m.getName();
    }
}
