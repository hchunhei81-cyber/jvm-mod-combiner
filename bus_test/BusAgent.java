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

    static volatile FabricBindingState fabricBindingState =
        FabricBindingState.UNINITIALIZED;
    static final java.util.concurrent.atomic.AtomicBoolean fabricSamplePending =
        new java.util.concurrent.atomic.AtomicBoolean(false);
    static volatile String lastFabricState = null;
    static volatile long lastFabricSendAt = 0L;
    static volatile boolean fabricSampleErrorPrinted = false;

    static Method getXMethod, getYMethod, getZMethod;
    static Method getYRotMethod, getXRotMethod, isSprintingMethod;

    // NeoForge
    static volatile Object nfMcInstance = null;
    static volatile Class<?> nfMcClass = null;
    static volatile Object remotePlayer = null;
    static volatile Object remotePlayerLevel = null;

    // NeoForge mirror lifecycle state.
    static volatile boolean dataBusConnected = false;
    static volatile boolean nfReconnectCleanupPending = false;
    static volatile long dataBusGeneration = 0L;
    static volatile long lastFabricReceiveAt = 0L;
    static final long FABRIC_TIMEOUT_MS = 3000L;
    static final java.util.concurrent.atomic.AtomicBoolean nfRemovalPending =
        new java.util.concurrent.atomic.AtomicBoolean(false);

    static volatile boolean nfInitialized = false;

    static volatile boolean nfMcFoundLogged = false;
    static volatile boolean nfLevelReadyLogged = false;
    static volatile boolean nfPlayerReadyLogged = false;
    static volatile boolean nfProbeErrorPrinted = false;

    static final java.util.concurrent.atomic.AtomicBoolean nfProbePending =
        new java.util.concurrent.atomic.AtomicBoolean(false);


    static final java.util.UUID remotePlayerUuid = java.util.UUID.nameUUIDFromBytes(
            "jvm-mod-combiner:fabric-mirror".getBytes(java.nio.charset.StandardCharsets.UTF_8));

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        inst = instrumentation;
        myName = agentArgs != null ? agentArgs.toLowerCase().replace("-sandbox", "") : "unknown";
        System.out.println("[BusAgent:" + myName + "] premain");

        Thread t = new Thread(BusAgent::runBus);
        t.setDaemon(true);
        t.start();

        if (myName.equals("fabric")) {
            Thread monitor = new Thread(BusAgent::monitorFabricPlayer);
            monitor.setDaemon(true);
            monitor.start();
        } else if (myName.equals("neoforge")) {
            Thread monitor = new Thread(
                BusAgent::monitorNeoForgePlayer,
                "BusAgent-NeoForge-Monitor"
            );
            monitor.setDaemon(true);
            monitor.start();
        }
    }

    static void runBus() {
        String peer = myName.equals("fabric") ? "neoforge" : "fabric";

        while (!Thread.currentThread().isInterrupted()) {
            try (Socket socket = new Socket()) {
                socket.connect(
                    new InetSocketAddress("127.0.0.1", 25565),
                    3000
                );

                socket.setKeepAlive(true);
                socket.setTcpNoDelay(true);

                try (
                    BufferedReader in = new BufferedReader(
                        new InputStreamReader(
                            socket.getInputStream(),
                            java.nio.charset.StandardCharsets.UTF_8
                        )
                    );
                    PrintWriter writer = new PrintWriter(
                        new OutputStreamWriter(
                            socket.getOutputStream(),
                            java.nio.charset.StandardCharsets.UTF_8
                        ),
                        true
                    )
                ) {
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
                        "[BusAgent:" + myName + "] Connected to DataBus"
                    );
                    if (myName.equals("neoforge")) {
                        scheduleRemotePlayerRemoval(
                            "bus-reconnected",
                            dataBusGeneration,
                            2,
                            0L
                        );
                    }


                    String line;

                    while ((line = in.readLine()) != null) {
                        if (!line.contains("\"type\":\"player_move\"")) {
                            continue;
                        }

                        java.util.regex.Matcher matcher =
                            java.util.regex.Pattern.compile(
                                "\"from\"\\s*:\\s*\"([^\"]+)\""
                            ).matcher(line);

                        if (!matcher.find()) {
                            continue;
                        }

                        String source = matcher.group(1);

                        if (!peer.equals(source)) {
                            continue;
                        }

                        if (myName.equals("neoforge")) {
                            synchronized (BusAgent.class) {
                                lastFabricReceiveAt = System.currentTimeMillis();
                            }
                            handleNeoForgeSync(line);
                        }

                        // Fabric 接收处理器将在第二轮实现。
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
                    "] Bus disconnected: " + e.getMessage()
                );

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
                        "net.fabricmc.loader.api.FabricLoader"
                    );

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

                    fabricMcInstance = instance;

                    // Minecraft 1.21.4: flk.t is the local player.
                    fabricPlayerField = instance.getClass().getField("t");

                    // Minecraft inherits execute(Runnable) from bra.
                    fabricExecuteMethod = instance.getClass().getMethod(
                        "execute",
                        Runnable.class
                    );

                    System.out.println(
                        "[BusAgent:fabric][INIT] SUCCESS: " +
                        instance.getClass().getName()
                    );

                    System.out.println(
                        "[BusAgent:fabric][INIT] player field=" +
                        fabricPlayerField.getName() +
                        ", executor=" +
                        fabricExecuteMethod.getDeclaringClass().getName()
                    );
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
                                            "[BusAgent:fabric][ERROR] player sample failed:"
                                        );
                                        e.printStackTrace(System.err);
                                    }
                                }

                                // M1: sample hostile mobs independently of player sampling.
                                try {
                MonsterSync.sample(fabricMcInstance);
                                } catch (Throwable mobError) {
                                    System.err.println(
                                        "[BusAgent:fabric][ERROR] MonsterSync invocation failed: " +
                                        mobError
                                    );
                                } finally {
                                    fabricSamplePending.set(false);
                                }
                            }
                        );
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
                    "[BusAgent:fabric][ERROR] monitor failed: " + e
                );

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

        String state =
            Double.toString(x) + "|" +
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

    static void sendFabricPlayer(
        double x,
        double y,
        double z,
        float yaw,
        float pitch,
        boolean sprint,
        String state,
        long now
    ) {
        PrintWriter writer = out;

        if (writer == null) {
            return;
        }

        String json =
            "{\"to\":\"neoforge\",\"type\":\"player_move\"," +
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
                    "[BusAgent:fabric][SEND][ERROR] TCP write failed"
                );
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
            " sprint=" + sprint
        );
    }



    static void monitorNeoForgePlayer() {
        System.out.println(
            "[BusAgent:neoforge][MONITOR] thread started"
        );

        while (!Thread.currentThread().isInterrupted()) {
            try {
                initNeoForgeMinecraft();

                checkRemotePlayerLifecycle();

                Object mc = nfMcInstance;

                if (mc != null) {
                    if (!nfMcFoundLogged) {
                        nfMcFoundLogged = true;
                        System.out.println(
                            "[BusAgent:neoforge][INIT] Minecraft instance found"
                        );
                    }

                    if (nfProbePending.compareAndSet(false, true)) {
                        try {
                            nfMcClass.getMethod(
                                "execute",
                                Runnable.class
                            ).invoke(mc, (Runnable) () -> {
                                try {
                                    Object level =
                                        nfMcClass.getField("level").get(mc);

                                    Object player =
                                        nfMcClass.getField("player").get(mc);

                                    boolean levelReady = level != null;
                                    boolean playerReady =
                                        levelReady && player != null;

                                    synchronized (BusAgent.class) {
                                        if (levelReady != nfLevelReadyLogged) {
                                            nfLevelReadyLogged = levelReady;

                                            System.out.println(
                                                levelReady
                                                ? "[BusAgent:neoforge][INIT] ClientLevel ready"
                                                : "[BusAgent:neoforge][WAIT] ClientLevel unavailable"
                                            );
                                        }

                                        if (playerReady != nfPlayerReadyLogged) {
                                            nfPlayerReadyLogged = playerReady;

                                            System.out.println(
                                                playerReady
                                                ? "[BusAgent:neoforge][LIVE] LocalPlayer ready"
                                                : "[BusAgent:neoforge][WAIT] LocalPlayer unavailable"
                                            );
                                        }
                                    }

                                    // NeoForge is a receiver-only client.
                                    // Fabric is the sole authoritative player-state source.
                                    // Never sample or transmit NeoForge LocalPlayer state.

                                } catch (Throwable e) {
                                    if (!nfProbeErrorPrinted) {
                                        nfProbeErrorPrinted = true;
                                        System.err.println(
                                            "[BusAgent:neoforge][ERROR] player probe failed: " + e
                                        );
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
                                    "[BusAgent:neoforge][ERROR] execute failed: " + e
                                );
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
                    "[BusAgent:neoforge][ERROR] monitor failed: " + e
                );

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
                    loader
                );

                Object discarded = Enum.valueOf(
                    (Class<Enum>) reasonClass.asSubclass(Enum.class),
                    "DISCARDED"
                );

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
                            .invoke(player)
                    );
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
                + "Mirror removed reason=" + reason
            );
        } else {
            System.err.println(
                "[BusAgent:neoforge][LIFECYCLE][ERROR] "
                + "Mirror removal failed; reference retained. reason="
                + reason + " error=" + removalError
            );
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
            "[BusAgent:neoforge][LIFECYCLE] DataBus disconnected: " + reason
        );

        scheduleRemotePlayerRemoval(
            "bus-disconnected",
            generation,
            0,
            0L
        );
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
                0L
            );
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
                0L
            );
            return;
        }

        if (lastReceive > 0L &&
            System.currentTimeMillis() - lastReceive > FABRIC_TIMEOUT_MS) {

            scheduleRemotePlayerRemoval(
                "fabric-heartbeat-timeout",
                generation,
                1,
                lastReceive
            );
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
        long expectedReceiveAt
    ) {
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
                                    System.currentTimeMillis() - expectedReceiveAt
                                        <= FABRIC_TIMEOUT_MS) {
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
                            + "Scheduled cleanup failed: " + e
                        );
                    } finally {
                        nfRemovalPending.set(false);
                    }
                }
            );

        } catch (Throwable e) {
            nfRemovalPending.set(false);

            System.err.println(
                "[BusAgent:neoforge][LIFECYCLE][ERROR] "
                + "Could not schedule cleanup: " + e
            );
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

    
    static boolean bindMethods(Class<?> clazz) {
        if (fabricBindingState != FabricBindingState.UNINITIALIZED) {
            return fabricBindingState == FabricBindingState.READY;
        }

        System.out.println(
            "[BusAgent:fabric] binding Minecraft 1.21.4 methods: " +
            clazz.getName()
        );

        try {
            // Resolve every method into local variables first.
            // Do not publish a partially initialized binding.
            Method x = clazz.getMethod("dA");
            Method y = clazz.getMethod("dC");
            Method z = clazz.getMethod("dG");
            Method yaw = clazz.getMethod("dL");
            Method pitch = clazz.getMethod("dN");
            Method sprint = clazz.getMethod("cj");

            // Commit only after all six lookups succeed.
            getXMethod = x;
            getYMethod = y;
            getZMethod = z;
            getYRotMethod = yaw;
            getXRotMethod = pitch;
            isSprintingMethod = sprint;

            fabricBindingState = FabricBindingState.READY;

            System.out.println(
                "[BusAgent:fabric] binding result X=" + nameOf(x) +
                " Y=" + nameOf(y) +
                " Z=" + nameOf(z) +
                " Yaw=" + nameOf(yaw) +
                " Pitch=" + nameOf(pitch) +
                " Sprint=" + nameOf(sprint)
            );

            System.out.println(
                "[BusAgent:fabric] method binding state: READY"
            );

            return true;

        } catch (NoSuchMethodException | SecurityException e) {
            fabricBindingState = FabricBindingState.FAILED;

            System.err.println(
                "[BusAgent:fabric][ERROR] 1.21.4 method binding failed; " +
                "binding permanently disabled"
            );
            e.printStackTrace(System.err);

            System.err.println(
                "[BusAgent:fabric] method binding state: FAILED"
            );

            return false;
        }
    }


    static String nameOf(Method m) {
        return m == null ? "null" : m.getDeclaringClass().getSimpleName() + "." + m.getName();
    }
}
