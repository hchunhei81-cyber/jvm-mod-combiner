import java.io.*;
import java.net.*;
import java.lang.instrument.*;
import java.lang.reflect.*;
import java.util.*;

public class BusAgent {
    static volatile PrintWriter out;
    static String myName;
    static Instrumentation inst;

    // Fabric
    static Object fabricMcInstance = null;
    static Field fabricPlayerField = null;
    static volatile boolean fabricStructureDumped = false;

    static volatile java.lang.reflect.Method fabricExecuteMethod = null;

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
    static volatile Object remotePlayer = null;
    static volatile Object remotePlayerLevel = null;
    static final java.util.concurrent.ConcurrentHashMap<java.util.UUID, Object> remoteMonsters = new java.util.concurrent.ConcurrentHashMap<>();
    static volatile Object remoteMonsterLevel = null;
    static volatile int nextRemoteMonsterId = 1_800_000_000;
    static final java.util.concurrent.atomic.AtomicBoolean monsterErrorPrinted = new java.util.concurrent.atomic.AtomicBoolean(
            false);
    // NeoForge mirror lifecycle state.
    static volatile boolean dataBusConnected = false;
    static volatile boolean nfReconnectCleanupPending = false;
    static volatile long dataBusGeneration = 0L;
    static volatile long lastFabricReceiveAt = 0L;
    static final long FABRIC_TIMEOUT_MS = 3000L;
    static final java.util.concurrent.atomic.AtomicBoolean nfRemovalPending = new java.util.concurrent.atomic.AtomicBoolean(
            false);

    static volatile boolean nfInitialized = false;

    static volatile boolean nfMcFoundLogged = false;
    static volatile boolean nfLevelReadyLogged = false;
    static volatile boolean nfPlayerReadyLogged = false;
    static volatile boolean nfProbeErrorPrinted = false;

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

        while (!Thread.currentThread().isInterrupted()) {
            try (Socket socket = new Socket()) {
                socket.connect(
                        new InetSocketAddress("127.0.0.1", 25565),
                        3000);

                socket.setKeepAlive(true);
                socket.setTcpNoDelay(true);

                try (
                        BufferedReader in = new BufferedReader(
                                new InputStreamReader(
                                        socket.getInputStream(),
                                        java.nio.charset.StandardCharsets.UTF_8));
                        PrintWriter writer = new PrintWriter(
                                new OutputStreamWriter(
                                        socket.getOutputStream(),
                                        java.nio.charset.StandardCharsets.UTF_8),
                                true)) {
                    synchronized (BusAgent.class) {
                        out = writer;
                        if (myName.equals("neoforge")) {
                            dataBusGeneration++;
                            dataBusConnected = true;
                            nfReconnectCleanupPending = true;
                            lastFabricReceiveAt = 0L;
                        }
                    }
                    writer.println("{\"register\":\"" + myName + "\"}");

                    System.out.println(
                            "[BusAgent:" + myName + "] Connected to DataBus");
                    if (myName.equals("neoforge")) {
                        scheduleRemotePlayerRemoval(
                                "bus-reconnected",
                                dataBusGeneration,
                                2,
                                0L);
                    }

                    String line;

                    while ((line = in.readLine()) != null) {
                        java.util.regex.Matcher typeMatcher = java.util.regex.Pattern.compile(
                                "\"type\"\\s*:\\s*\"([^\"]+)\"")
                                .matcher(line);

                        if (!typeMatcher.find()) {
                            continue;
                        }

                        java.util.regex.Matcher fromMatcher = java.util.regex.Pattern.compile(
                                "\"from\"\\s*:\\s*\"([^\"]+)\"")
                                .matcher(line);

                        if (!fromMatcher.find() ||
                                peer.isEmpty() ||
                                !peer.equals(fromMatcher.group(1))) {
                            continue;
                        }

                        String incomingType = typeMatcher.group(1);

                        if ("fabric".equals(myName)) {
                            if ("monster_ack".equals(incomingType)) {
                                try {
                                    java.util.UUID ackUuid =
                                            java.util.UUID.fromString(
                                                    readMonsterJsonString(line, "uuid"));

                                    long ackSeq =
                                            readMonsterJsonLong(line, "seq");

                                    MonsterSync.acknowledge(ackUuid, ackSeq);
                                } catch (Throwable error) {
                                    System.err.println(
                                            "[BusAgent:fabric][MONSTER][ACK][ERROR] " +
                                                    "Invalid ACK: " + error.getMessage());
                                }
                            }

                            // Fabric accepts only business ACKs from NeoForge.
                            continue;
                        }

                        if (!"neoforge".equals(myName)) {
                            continue;
                        }

                        if ("player_move".equals(incomingType)) {
                            synchronized (BusAgent.class) {
                                lastFabricReceiveAt = System.currentTimeMillis();
                            }
                            handleNeoForgeSync(line);
                        } else if ("monster_spawn".equals(typeMatcher.group(1)) ||
                                "monster_update".equals(typeMatcher.group(1))) {
                            handleNeoForgeMonsterSync(line);
                        }
                    }
                } finally {
                    out = null;
                    markDataBusDisconnected("stream-ended");
                }

            } catch (Exception e) {
                out = null;
                markDataBusDisconnected("connection-error");
                System.out.println(
                        "[BusAgent:" + myName +
                                "] Bus disconnected: " + e.getMessage());

                try {
                    Thread.sleep(2000);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }
    }

    static void monitorFabricPlayer() {
        System.out.println("[BusAgent:fabric] monitor thread started");

        while (!Thread.currentThread().isInterrupted()) {
            try {
                if (fabricMcInstance == null) {
                    Class<?> loaderClass = Class.forName(
                            "net.fabricmc.loader.api.FabricLoader");

                    Object loader = loaderClass
                            .getMethod("getInstance")
                            .invoke(null);

                    Object instance = loaderClass
                            .getMethod("getGameInstance")
                            .invoke(loader);

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
                        "[BusAgent:fabric][MONSTER][SEND][ERROR] " +
                                "TCP write failed");
                return false;
            }
        }

        System.out.println(
                "[BusAgent:fabric][MONSTER][SEND] message written");

        return true;
    }

    static boolean sendNeoForgeMonsterAck(
            java.util.UUID uuid,
            long seq) {
        if (!"neoforge".equals(myName) ||
                uuid == null ||
                seq <= 0L) {
            return false;
        }

        String json =
                "{\"to\":\"fabric\"," +
                "\"type\":\"monster_ack\"," +
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
                        "[BusAgent:neoforge][MONSTER][ACK][ERROR] " +
                                "TCP write failed");
                return false;
            }
        }

        System.out.println(
                "[BusAgent:neoforge][MONSTER][ACK] uuid=" +
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
                        "[BusAgent:fabric][SEND][ERROR] TCP write failed");
                return;
            }
        }

        lastFabricState = state;
        lastFabricSendAt = now;

        System.out.println(
                "[BusAgent:fabric][SEND] x=" + x +
                        " y=" + y +
                        " z=" + z +
                        " yaw=" + yaw +
                        " pitch=" + pitch +
                        " sprint=" + sprint);
    }

    static void monitorNeoForgePlayer() {
        System.out.println(
                "[BusAgent:neoforge][MONITOR] thread started");

        while (!Thread.currentThread().isInterrupted()) {
            try {
                initNeoForgeMinecraft();

                checkRemotePlayerLifecycle();

                Object mc = nfMcInstance;

                if (mc != null) {
                    if (!nfMcFoundLogged) {
                        nfMcFoundLogged = true;
                        System.out.println(
                                "[BusAgent:neoforge][INIT] Minecraft instance found");
                    }

                    if (nfProbePending.compareAndSet(false, true)) {
                        try {
                            nfMcClass.getMethod(
                                    "execute",
                                    Runnable.class).invoke(mc, (Runnable) () -> {
                                        try {
                                            Object level = nfMcClass.getField("level").get(mc);

                                            Object player = nfMcClass.getField("player").get(mc);

                                            boolean levelReady = level != null;
                                            boolean playerReady = levelReady && player != null;

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
        if (nfMcInstance != null)
            return;
        try {
            if (nfMcClass == null) {
                for (Class<?> c : inst.getAllLoadedClasses()) {
                    if (c.getName().equals("net.minecraft.client.Minecraft")) {
                        nfMcClass = c;
                        break;
                    }
                }
            }
            if (nfMcClass == null) {
                return;
            }
            nfMcInstance = nfMcClass.getMethod("getInstance").invoke(null);
            if (nfMcInstance != null) {
                System.out.println("[BusAgent:neoforge] ✅ 成功锁定 Minecraft 实例！");
                nfInitialized = true;
            }
        } catch (Throwable e) {
            System.out.println("[BusAgent:neoforge] init failed: " + e);
        }
    }

    static void handleNeoForgeSync(String json) {
        try {
            if (!nfInitialized || nfMcInstance == null) {
                initNeoForgeMinecraft();
                if (nfMcInstance == null)
                    return;
            }

            double x = readJsonNumber(json, "x");
            double y = readJsonNumber(json, "y");
            double z = readJsonNumber(json, "z");
            float yaw = (float) readJsonNumber(json, "yaw");
            float pitch = (float) readJsonNumber(json, "pitch");
            boolean sprint = readJsonBool(json, "sprint");

            final long syncGeneration;
            synchronized (BusAgent.class) {
                if (!dataBusConnected) {
                    return;
                }
                syncGeneration = dataBusGeneration;
            }

            Object mc = nfMcInstance;
            nfMcClass.getMethod("execute", Runnable.class).invoke(mc, (Runnable) () -> {
                try {
                    synchronized (BusAgent.class) {
                        if (!dataBusConnected ||
                                dataBusGeneration != syncGeneration) {
                            return;
                        }
                    }

                    Object level = nfMcClass.getField("level").get(mc);
                    if (level == null)
                        return;
                    ClassLoader loader = nfMcClass.getClassLoader();

                    Class<?> levelClass = Class.forName("net.minecraft.client.multiplayer.ClientLevel", true, loader);
                    Class<?> playerClass = Class.forName("net.minecraft.client.player.RemotePlayer", true, loader);
                    Class<?> profileClass = Class.forName("com.mojang.authlib.GameProfile", true, loader);
                    Class<?> entityClass = Class.forName("net.minecraft.world.entity.Entity", true, loader);

                    Class<?> entityTypeClass = Class.forName(
                            "net.minecraft.world.entity.EntityType",
                            true,
                            loader);

                    Class<?> registryApiClass = Class.forName(
                            "net.minecraft.core.Registry",
                            true,
                            loader);

                    if (remotePlayer != null && remotePlayerLevel != level) {
                        removeRemotePlayerNow("world-changed");
                    }

                    if (remotePlayer == null) {
                        Object profile = profileClass.getConstructor(java.util.UUID.class, String.class)
                                .newInstance(remotePlayerUuid, "FabricMirror");
                        Object player = playerClass.getConstructor(levelClass, profileClass)
                                .newInstance(level, profile);

                        Method getEntity = levelClass.getMethod("getEntity", int.class);
                        Method setId = entityClass.getMethod("setId", int.class);
                        Method setUUID = entityClass.getMethod("setUUID", java.util.UUID.class);

                        int candidate = 1_900_000_000;
                        for (int i = 0; i < 10000; i++, candidate--) {
                            if (getEntity.invoke(level, candidate) == null)
                                break;
                        }

                        setId.invoke(player, candidate);
                        setUUID.invoke(player, remotePlayerUuid);

                        entityClass.getMethod("setPos", double.class, double.class, double.class)
                                .invoke(player, x, y, z);
                        entityClass.getMethod("setOldPosAndRot").invoke(player);

                        levelClass.getMethod("addEntity", entityClass).invoke(level, player);
                        remotePlayer = player;
                        remotePlayerLevel = level;

                        System.out.println("[BusAgent:neoforge] ✅ 成功创建镜像实体！ID=" + candidate);
                    } else {
                        entityClass.getMethod("setPos", double.class, double.class, double.class)
                                .invoke(remotePlayer, x, y, z);
                        entityClass.getMethod("setOldPosAndRot").invoke(remotePlayer);
                    }

                    Class<?> runtimeCls = remotePlayer.getClass();
                    try {
                        runtimeCls.getMethod("setYRot", float.class).invoke(remotePlayer, yaw);
                    } catch (Throwable ignored) {
                    }
                    try {
                        runtimeCls.getMethod("setXRot", float.class).invoke(remotePlayer, pitch);
                    } catch (Throwable ignored) {
                    }
                    try {
                        runtimeCls.getMethod("setYHeadRot", float.class).invoke(remotePlayer, yaw);
                    } catch (Throwable ignored) {
                    }
                    try {
                        runtimeCls.getMethod("setYBodyRot", float.class).invoke(remotePlayer, yaw);
                    } catch (Throwable ignored) {
                    }
                    try {
                        runtimeCls.getMethod("setSprinting", boolean.class).invoke(remotePlayer, sprint);
                    } catch (Throwable ignored) {
                    }
                    System.out.println(
                            "[BusAgent:neoforge] 同步 -> (" + x + "," + y + "," + z + ") yaw=" + yaw + " pitch=" + pitch);

                } catch (Throwable e) {
                    System.out.println("[BusAgent:neoforge] Runnable err: " + e);
                }
            });
        } catch (Throwable e) {
            System.out.println("[BusAgent:neoforge] handle err: " + e);
        }
    }

    static void handleNeoForgeMonsterSync(String json) {
        try {
            if (!nfInitialized || nfMcInstance == null) {
                initNeoForgeMinecraft();

                if (nfMcInstance == null) {
                    return;
                }
            }

            java.util.UUID uuid = java.util.UUID.fromString(
                    readMonsterJsonString(json, "uuid"));

            long seq = readMonsterJsonLong(json, "seq");

            String entityTypeId = readMonsterJsonString(json, "entity_type");

            double x = readMonsterJsonNumber(json, "x");
            double y = readMonsterJsonNumber(json, "y");
            double z = readMonsterJsonNumber(json, "z");

            float yaw = (float) readMonsterJsonNumber(json, "yaw");
            float pitch = (float) readMonsterJsonNumber(json, "pitch");

            float health = (float) readMonsterJsonNumber(json, "health");
            float maxHealth = (float) readMonsterJsonNumber(json, "max_health");

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

            final long generation;

            synchronized (BusAgent.class) {
                if (!dataBusConnected) {
                    return;
                }

                generation = dataBusGeneration;
            }

            Object mc = nfMcInstance;

            nfMcClass.getMethod(
                    "execute",
                    Runnable.class).invoke(mc, (Runnable) () -> {
                        try {
                            synchronized (BusAgent.class) {
                                if (!dataBusConnected ||
                                        dataBusGeneration != generation) {
                                    return;
                                }
                            }

                            Object level = nfMcClass.getField("level").get(mc);

                            if (level == null) {
                                return;
                            }

                            ClassLoader loader = nfMcClass.getClassLoader();

                            Class<?> entityClass = Class.forName(
                                    "net.minecraft.world.entity.Entity",
                                    true,
                                    loader);

                            Class<?> livingClass = Class.forName(
                                    "net.minecraft.world.entity.LivingEntity",
                                    true,
                                    loader);

                            Class<?> levelClass = Class.forName(
                                    "net.minecraft.world.level.Level",
                                    true,
                                    loader);

                            Class<?> clientLevelClass = Class.forName(
                                    "net.minecraft.client.multiplayer.ClientLevel",
                                    true,
                                    loader);

                            Class<?> entityTypeClass = Class.forName(
                                    "net.minecraft.world.entity.EntityType",
                                    true,
                                    loader);

                            Class<?> registryClass = Class.forName(
                                    "net.minecraft.core.registries.BuiltInRegistries",
                                    true,
                                    loader);

                            Class<?> registryApiClass = Class.forName(
                                    "net.minecraft.core.Registry",
                                    true,
                                    loader);

                            Class<?> resourceLocationClass = Class.forName(
                                    "net.minecraft.resources.ResourceLocation",
                                    true,
                                    loader);

                            Class<?> spawnReasonClass = Class.forName(
                                    "net.minecraft.world.entity.EntitySpawnReason",
                                    true,
                                    loader);

                            // World changed: discard only monster mirrors.
                            if (remoteMonsterLevel != null &&
                                    remoteMonsterLevel != level) {

                                for (Object old : remoteMonsters.values()) {
                                    try {
                                        old.getClass()
                                                .getMethod("discard")
                                                .invoke(old);
                                    } catch (Throwable ignored) {
                                    }
                                }

                                remoteMonsters.clear();
                                remoteMonsterLevel = null;

                                System.out.println(
                                        "[BusAgent:neoforge][MONSTER] " +
                                                "Cleared mirrors after world change");
                            }

                            remoteMonsterLevel = level;

                            Object entity = remoteMonsters.get(uuid);

                            if (entity != null) {
                                boolean removed = Boolean.TRUE.equals(
                                        entityClass.getMethod("isRemoved")
                                                .invoke(entity));

                                if (removed) {
                                    remoteMonsters.remove(uuid, entity);
                                    entity = null;
                                }
                            }

                            if (entity == null) {
                                Object resourceLocation = resourceLocationClass
                                        .getMethod("parse", String.class)
                                        .invoke(null, entityTypeId);

                                Object registry = registryClass.getField("ENTITY_TYPE")
                                        .get(null);

                                Object optional = registryApiClass
                                        .getMethod(
                                                "getOptional",
                                                resourceLocationClass)
                                        .invoke(registry, resourceLocation);

                                if (!(optional instanceof java.util.Optional<?>)) {
                                    throw new IllegalStateException(
                                            "Entity registry lookup returned invalid result");
                                }

                                java.util.Optional<?> found = (java.util.Optional<?>) optional;

                                if (found.isEmpty()) {
                                    System.err.println(
                                            "[BusAgent:neoforge][MONSTER][SKIP] " +
                                                    "Unknown entity type: " + entityTypeId);
                                    return;
                                }

                                Object entityType = found.get();

                                Object spawnReason = Enum.valueOf(
                                        (Class<Enum>) spawnReasonClass.asSubclass(Enum.class),
                                        "COMMAND");

                                entity = entityTypeClass
                                        .getMethod(
                                                "create",
                                                levelClass,
                                                spawnReasonClass)
                                        .invoke(entityType, level, spawnReason);

                                if (entity == null) {
                                    System.err.println(
                                            "[BusAgent:neoforge][MONSTER][SKIP] " +
                                                    "Entity creation returned null: " +
                                                    entityTypeId);
                                    return;
                                }

                                int candidate = nextRemoteMonsterId;

                                Method getEntity = clientLevelClass
                                        .getMethod("getEntity", int.class);

                                Method setId = entityClass.getMethod("setId", int.class);

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

                                nextRemoteMonsterId = candidate - 1;

                                setId.invoke(entity, candidate);

                                entityClass.getMethod(
                                        "setUUID",
                                        java.util.UUID.class).invoke(entity, uuid);

                                entityClass.getMethod(
                                        "setPos",
                                        double.class,
                                        double.class,
                                        double.class).invoke(entity, x, y, z);

                                entityClass.getMethod(
                                        "setOldPosAndRot").invoke(entity);

                                clientLevelClass.getMethod(
                                        "addEntity",
                                        entityClass).invoke(level, entity);

                                remoteMonsters.put(uuid, entity);

                                System.out.println(
                                        "[BusAgent:neoforge][MONSTER][SPAWN] " +
                                                "type=" + entityTypeId +
                                                " uuid=" + uuid +
                                                " id=" + candidate);
                            }

                            entityClass.getMethod(
                                    "setPos",
                                    double.class,
                                    double.class,
                                    double.class).invoke(entity, x, y, z);

                            entityClass.getMethod(
                                    "setYRot",
                                    float.class).invoke(entity, yaw);

                            entityClass.getMethod(
                                    "setXRot",
                                    float.class).invoke(entity, pitch);

                            entityClass.getMethod(
                                    "setYHeadRot",
                                    float.class).invoke(entity, yaw);

                            entityClass.getMethod(
                                    "setYBodyRot",
                                    float.class).invoke(entity, yaw);

                            entityClass.getMethod(
                                    "setOldPosAndRot").invoke(entity);

                            if (livingClass.isInstance(entity)) {
                                livingClass.getMethod(
                                        "setHealth",
                                        float.class).invoke(
                                                entity,
                                                Math.max(0.0f, Math.min(health, maxHealth)));
                            }

                            // Business ACK: only after creation/update and
                            // all requested state fields have been applied.
                            sendNeoForgeMonsterAck(uuid, seq);

                            if (!"monster_spawn".equals(
                                    readMonsterJsonString(json, "type"))) {
                                // Updates are intentionally quiet to avoid log flooding.
                            }

                        } catch (Throwable error) {
                            if (monsterErrorPrinted.compareAndSet(false, true)) {
                                System.err.println(
                                        "[BusAgent:neoforge][MONSTER][ERROR] " +
                                                "Monster main-thread handler failed");

                                error.printStackTrace(System.err);
                            }
                        }
                    });

        } catch (Throwable error) {
            if (monsterErrorPrinted.compareAndSet(false, true)) {
                System.err.println(
                        "[BusAgent:neoforge][MONSTER][ERROR] " +
                                "Monster message rejected");

                error.printStackTrace(System.err);
            }
        }
    }

    // Must only be called on the NeoForge Minecraft game thread.
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

                Object discarded = Enum.valueOf(
                        (Class<Enum>) reasonClass.asSubclass(Enum.class),
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
        }

        System.out.println(
                "[BusAgent:neoforge][LIFECYCLE] DataBus disconnected: " + reason);

        scheduleRemotePlayerRemoval(
                "bus-disconnected",
                generation,
                0,
                0L);
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
            mcClass.getMethod("execute", Runnable.class).invoke(
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

    static boolean readJsonBool(String json, String key) {
        return json.matches("(?s).*\"" + key + "\"\\s*:\\s*true.*");
    }

    static double readJsonNumber(String json, String key) {
        java.util.regex.Pattern p = java.util.regex.Pattern.compile(
                "\"" + key + "\"\\s*:\\s*(-?\\d+(?:\\.\\d+)?)");
        java.util.regex.Matcher m = p.matcher(json);
        if (m.find())
            return Double.parseDouble(m.group(1));
        return 0;
    }

    static String readMonsterJsonString(
            String json,
            String key) {
        java.util.regex.Matcher matcher = java.util.regex.Pattern.compile(
                "\"" + java.util.regex.Pattern.quote(key) +
                        "\"\\s*:\\s*\"([^\"]*)\"")
                .matcher(json);

        if (!matcher.find()) {
            throw new IllegalArgumentException(
                    "Missing JSON string field: " + key);
        }

        return matcher.group(1);
    }

    static double readMonsterJsonNumber(
            String json,
            String key) {
        java.util.regex.Matcher matcher = java.util.regex.Pattern.compile(
                "\"" + java.util.regex.Pattern.quote(key) +
                        "\"\\s*:\\s*" +
                        "(-?(?:\\d+(?:\\.\\d*)?|\\.\\d+)(?:[eE][+-]?\\d+)?)")
                .matcher(json);

        if (!matcher.find()) {
            throw new IllegalArgumentException(
                    "Missing JSON number field: " + key);
        }

        double value = Double.parseDouble(matcher.group(1));

        if (!Double.isFinite(value)) {
            throw new IllegalArgumentException(
                    "Non-finite JSON number: " + key);
        }

        return value;
    }

    static long readMonsterJsonLong(
            String json,
            String key) {
        java.util.regex.Matcher matcher = java.util.regex.Pattern.compile(
                "\"" + java.util.regex.Pattern.quote(key) +
                        "\"\\s*:\\s*([0-9]+)")
                .matcher(json);

        if (!matcher.find()) {
            throw new IllegalArgumentException(
                    "Missing or invalid JSON integer field: " + key);
        }

        long value = Long.parseLong(matcher.group(1));

        if (value <= 0L) {
            throw new IllegalArgumentException(
                    "JSON integer must be positive: " + key);
        }

        return value;
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
