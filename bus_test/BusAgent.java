import java.io.*;
import java.net.*;
import java.lang.instrument.*;
import java.lang.reflect.*;
import java.util.*;

public class BusAgent {
    static PrintWriter out;
    static String myName;
    static Instrumentation inst;

    // Fabric
    static Object fabricMcInstance = null;
    static Field fabricPlayerField = null;
    static volatile boolean fabricStructureDumped = false;
    static Method getXMethod, getYMethod, getZMethod;
    static Method getYRotMethod, getXRotMethod, isSprintingMethod;

    // NeoForge
    static volatile Object nfMcInstance = null;
    static volatile Class<?> nfMcClass = null;
    static volatile Object remotePlayer = null;
    static volatile Object remotePlayerLevel = null;
    static volatile boolean nfInitialized = false;

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
        }
    }

    static void runBus() {
        String peer = myName.equals("fabric") ? "neoforge" : "fabric";
        try (Socket socket = new Socket("127.0.0.1", 25565);
                BufferedReader in = new BufferedReader(new InputStreamReader(socket.getInputStream()))) {
            out = new PrintWriter(socket.getOutputStream(), true);
            out.println("{\"register\":\"" + myName + "\"}");

            String line;
            while ((line = in.readLine()) != null) {
                if (line.contains("\"type\":\"player_move\"") && line.contains("\"source\":\"" + peer + "\"")) {
                    if (myName.equals("neoforge")) {
                        handleNeoForgeSync(line);
                    }
                }
            }
        } catch (Exception e) {
            System.out.println("[BusAgent:" + myName + "] Bus failed: " + e);
        }
    }

    static void monitorFabricPlayer() {
        System.out.println("[BusAgent:fabric] monitor thread started");

        long lastScanLog = 0;
        boolean errorPrinted = false;

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
                        long now = System.currentTimeMillis();

                        if (now - lastScanLog >= 3000) {
                            System.out.println(
                                "[BusAgent:fabric][WAIT] Minecraft instance not ready"
                            );
                            lastScanLog = now;
                        }

                        Thread.sleep(250);
                        continue;
                    }

                    fabricMcInstance = instance;

                    System.out.println(
                        "[BusAgent:fabric][INIT] SUCCESS: " +
                        instance.getClass().getName()
                    );
                }

                if (!fabricStructureDumped) {
                    synchronized (BusAgent.class) {
                        if (!fabricStructureDumped) {
                            System.out.println(
                                "[BusAgent:fabric][STRUCT] Minecraft instance = " +
                                fabricMcInstance.getClass().getName()
                            );

                            Class<?> scanClass = fabricMcInstance.getClass();

                            while (scanClass != null &&
                                   scanClass != Object.class) {

                                System.out.println(
                                    "[BusAgent:fabric][STRUCT] CLASS " +
                                    scanClass.getName()
                                );

                                for (Field field : scanClass.getDeclaredFields()) {
                                    try {
                                        field.setAccessible(true);
                                        Object value = field.get(fabricMcInstance);

                                        System.out.println(
                                            "[BusAgent:fabric][STRUCT] FIELD " +
                                            field.getName() +
                                            " type=" + field.getType().getName() +
                                            " value=" +
                                            (value == null
                                                ? "null"
                                                : value.getClass().getName())
                                        );
                                    } catch (Throwable e) {
                                        System.out.println(
                                            "[BusAgent:fabric][STRUCT] FIELD " +
                                            field.getName() + " ERROR " + e
                                        );
                                    }
                                }

                                scanClass = scanClass.getSuperclass();
                            }

                            fabricStructureDumped = true;
                        }
                    }
                }

                /*
                 * 玩家字段扫描和位置发送逻辑暂时保留在原方法中。
                 * 这里先验证 Minecraft 实例初始化和结构扫描。
                 */
                Thread.sleep(250);

            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;

            } catch (Throwable e) {
                if (!errorPrinted) {
                    errorPrinted = true;
                    System.err.println(
                        "[BusAgent:fabric][ERROR] monitorFabricPlayer failed:"
                    );
                    e.printStackTrace(System.err);
                }

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
                System.out.println("[BusAgent:neoforge] 还没找到 Minecraft 类");
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

            Object mc = nfMcInstance;
            nfMcClass.getMethod("execute", Runnable.class).invoke(mc, (Runnable) () -> {
                try {
                    Object level = nfMcClass.getField("level").get(mc);
                    if (level == null)
                        return;
                    ClassLoader loader = nfMcClass.getClassLoader();

                    Class<?> levelClass = Class.forName("net.minecraft.client.multiplayer.ClientLevel", true, loader);
                    Class<?> playerClass = Class.forName("net.minecraft.client.player.RemotePlayer", true, loader);
                    Class<?> profileClass = Class.forName("com.mojang.authlib.GameProfile", true, loader);
                    Class<?> entityClass = Class.forName("net.minecraft.world.entity.Entity", true, loader);

                    if (remotePlayer != null && remotePlayerLevel != level) {
                        remotePlayer = null;
                        remotePlayerLevel = null;
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

    static void bindMethods(Class<?> clazz) {
        System.out.println("[BusAgent:fabric] bindMethods: " + clazz.getName());

        getXMethod = null;
        getYMethod = null;
        getZMethod = null;
        getYRotMethod = null;
        getXRotMethod = null;
        isSprintingMethod = null;

        Class<?> c = clazz;
        int depth = 0;

        while (c != null && c != Object.class && depth < 12) {
            for (Method m : c.getDeclaredMethods()) {
                if (m.getParameterCount() != 0)
                    continue;

                String name = m.getName();
                Class<?> rt = m.getReturnType();

                try {
                    m.setAccessible(true);
                } catch (Throwable ignored) {
                }

                if (rt == double.class || rt == Double.class) {
                    if (name.equals("getX"))
                        getXMethod = m;
                    else if (name.equals("getY"))
                        getYMethod = m;
                    else if (name.equals("getZ"))
                        getZMethod = m;
                }

                if (rt == float.class || rt == Float.class) {
                    if (name.equals("getYRot"))
                        getYRotMethod = m;
                    else if (name.equals("getXRot"))
                        getXRotMethod = m;
                }

                if ((rt == boolean.class || rt == Boolean.class) &&
                        name.toLowerCase(Locale.ROOT).contains("sprint")) {
                    isSprintingMethod = m;
                }
            }

            c = c.getSuperclass();
            depth++;
        }

        System.out.println("[BusAgent:fabric] 严格绑定结果 X=" +
                nameOf(getXMethod) + " Y=" + nameOf(getYMethod) +
                " Z=" + nameOf(getZMethod) +
                " YRot=" + nameOf(getYRotMethod) +
                " XRot=" + nameOf(getXRotMethod) +
                " Sprint=" + nameOf(isSprintingMethod));

        if (getXMethod == null || getYMethod == null || getZMethod == null) {
            System.out.println(
                    "[BusAgent:fabric][ERROR] 无法确认 X/Y/Z getter；已禁止猜测绑定。");
        }
    }

    static String nameOf(Method m) {
        return m == null ? "null" : m.getDeclaringClass().getSimpleName() + "." + m.getName();
    }
}
