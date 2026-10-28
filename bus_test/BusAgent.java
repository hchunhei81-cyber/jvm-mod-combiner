import java.io.*;
import java.net.*;
import java.lang.instrument.*;
import java.lang.reflect.*;
import java.util.*;

public class BusAgent {
    static PrintWriter out;
    static String myName;
    static Instrumentation inst;

    static Object fabricMcInstance = null;
    static Field fabricPlayerField = null;
    static Method getXMethod, getYMethod, getZMethod;
    // FABRIC_DIAGNOSTICS_PATCH
    static volatile boolean fabricErrorLogged = false;
    static volatile boolean fabricFieldScanLogged = false;


    static volatile Object nfMcInstance = null;
    static volatile Class<?> nfMcClass = null;
    static volatile Object remotePlayer = null;
    static volatile Object remotePlayerLevel = null;
    static volatile boolean nfDiagInstanceLogged = false;
    static volatile boolean nfDiagLevelLogged = false;
    static volatile boolean nfDiagWorldReadyLogged = false;
    static volatile boolean nfDiagScheduleLogged = false;

    static final java.util.UUID remotePlayerUuid =
        java.util.UUID.nameUUIDFromBytes(
            "jvm-mod-combiner:fabric-mirror".getBytes(java.nio.charset.StandardCharsets.UTF_8)
        );

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        inst = instrumentation;
        myName = agentArgs != null ? agentArgs.toLowerCase().replace("-sandbox", "") : "unknown";
        System.out.println("[BusAgent:" + myName + "] premain");

        if (myName.equals("neoforge")) {
            inst.addTransformer(new ClassFileTransformer() {
                @Override
                public byte[] transform(ClassLoader loader, String className, Class<?> classBeingRedefined, java.security.ProtectionDomain protectionDomain, byte[] classfileBuffer) {
                    if (className != null && className.equals("net/minecraft/client/Minecraft")) {
                        final ClassLoader targetLoader = loader;
                        new Thread(() -> {
                            try {
                                for (int attempt = 1; attempt <= 180; attempt++) {
                                    try {
                                        nfMcClass = Class.forName(
                                            "net.minecraft.client.Minecraft",
                                            false,
                                            targetLoader
                                        );

                                        Object candidate =
                                            nfMcClass.getMethod("getInstance").invoke(null);

                                        if (candidate != null) {
                                            nfMcInstance = candidate;
                                            System.out.println(
                                                "[BusAgent:neoforge] 成功锁定非空 Minecraft 实例；"
                                                + "attempt=" + attempt
                                                + "，class=" + candidate.getClass().getName()
                                                + "，classLoader=" + candidate.getClass().getClassLoader()
                                            );
                                            return;
                                        }

                                        if (attempt == 1 || attempt % 10 == 0) {
                                            System.out.println(
                                                "[BusAgent:neoforge] Minecraft 实例尚未就绪，继续重试；"
                                                + "attempt=" + attempt
                                            );
                                        }
                                    } catch (Throwable retryError) {
                                        if (attempt == 1 || attempt % 10 == 0) {
                                            System.err.println(
                                                "[BusAgent:neoforge] 获取实例重试失败；attempt="
                                                + attempt + "，错误=" + retryError
                                            );
                                        }
                                    }

                                    Thread.sleep(1000);
                                }

                                System.err.println(
                                    "[BusAgent:neoforge] 等待 180 秒后仍未获取到非空 Minecraft 实例。"
                                );
                            } catch (Throwable e) {
                                System.err.println(
                                    "[BusAgent:neoforge] 获取 Minecraft 实例失败：");
                                e.printStackTrace(System.err);
                            }
                        }).start();
                    }
                    return null;
                }
            });
        }

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
        int attempt = 0;
        long received = 0;

        while (!Thread.currentThread().isInterrupted()) {
            attempt++;
            try (Socket socket = new Socket()) {
                socket.connect(new InetSocketAddress("127.0.0.1", 25565), 3000);
                socket.setKeepAlive(true);

                try (BufferedReader in = new BufferedReader(
                        new InputStreamReader(socket.getInputStream(), java.nio.charset.StandardCharsets.UTF_8));
                     PrintWriter writer = new PrintWriter(
                        new OutputStreamWriter(socket.getOutputStream(), java.nio.charset.StandardCharsets.UTF_8), true)) {

                    out = writer;
                    writer.println("{\"register\":\"" + myName + "\"}");

                    if (writer.checkError()) {
                        throw new IOException("发送注册消息失败");
                    }

                    System.out.println("[BusAgent:" + myName
                        + "] DataBus 已连接并发送注册消息，peer=" + peer
                        + "，attempt=" + attempt);

                    String line;
                    while ((line = in.readLine()) != null) {
                        if (line.contains("\"type\":\"player_move\"")
                                && line.contains("\"source\":\"" + peer + "\"")) {
                            received++;
                            if (received == 1 || received % 20 == 0) {
                                System.out.println("[BusAgent:" + myName
                                    + "] 已收到对端 player_move，累计=" + received
                                    + "，最近消息=" + line);
                            }

                            if (myName.equals("neoforge")) {
                                handleNeoForgeSync(line);
                            }
                        }
                    }

                    System.err.println("[BusAgent:" + myName
                        + "] DataBus 连接关闭，将尝试重新连接。");
                }
            } catch (Exception e) {
                System.err.println("[BusAgent:" + myName
                    + "] DataBus 连接/接收失败（attempt=" + attempt
                    + "）：" + e);
            } finally {
                out = null;
            }

            try {
                Thread.sleep(2000);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            }
        }
    }

    static void monitorFabricPlayer() {
        while (true) {
            try {
                if (fabricMcInstance == null) {
                    Class<?> loaderClass = Class.forName("net.fabricmc.loader.api.FabricLoader");
                    Object loader = loaderClass.getMethod("getInstance").invoke(null);
                    fabricMcInstance = loaderClass.getMethod("getGameInstance").invoke(loader);
                    if (fabricMcInstance != null) {
                        System.out.println("[BusAgent:fabric] Minecraft 实例类型: "
                            + fabricMcInstance.getClass().getName());
                    } else {
                        System.err.println("[BusAgent:fabric] getGameInstance() 返回 null");
                    }
                }

                if (fabricMcInstance != null && fabricPlayerField == null) {
                    try {
                        // 1.21.4 当前运行时：flk.t -> gkx（本地玩家）
                        Class<?> owner = fabricMcInstance.getClass();
                        Field candidate = null;

                        while (owner != null) {
                            try {
                                candidate = owner.getDeclaredField("t");
                                break;
                            } catch (NoSuchFieldException missing) {
                                owner = owner.getSuperclass();
                            }
                        }

                        if (candidate == null) {
                            throw new NoSuchFieldException(
                                "Minecraft 实例及父类中未找到字段 t"
                            );
                        }

                        if (!"gkx".equals(candidate.getType().getName())) {
                            throw new IllegalStateException(
                                "字段 t 类型与当前 1.21.4 运行时预期不符："
                                + candidate.getType().getName()
                            );
                        }

                        candidate.setAccessible(true);
                        fabricPlayerField = candidate;

                        System.out.println(
                            "[BusAgent:fabric] 已绑定玩家字段："
                            + owner.getName() + "." + candidate.getName()
                            + "，类型=" + candidate.getType().getName()
                        );
                    } catch (Throwable mappingError) {
                        // 只限制重复日志，不限制后续重试
                        if (!fabricFieldScanLogged) {
                            fabricFieldScanLogged = true;
                            System.err.println(
                                "[BusAgent:fabric] 玩家字段暂未绑定，将继续重试："
                                + mappingError
                            );
                        }
                    }
                }
                if (fabricPlayerField != null) {
                    Object player = fabricPlayerField.get(fabricMcInstance);
                    if (player != null) {
                        if (getXMethod == null || getYMethod == null || getZMethod == null) bindXYZ(player.getClass());
                        if (getXMethod != null && getYMethod != null && getZMethod != null) {
                            double x = (double) getXMethod.invoke(player);
                            double y = (double) getYMethod.invoke(player);
                            double z = (double) getZMethod.invoke(player);
                            String msg = String.format("{\"broadcast\":true,\"type\":\"player_move\",\"source\":\"fabric\",\"payload\":{\"x\":%.2f,\"y\":%.2f,\"z\":%.2f}}", x, y, z);
                            if (out != null) out.println(msg);
                            System.out.println("[BusAgent:fabric] 🚀 发送真实坐标: " + x + ", " + y + ", " + z);
                        }
                    }
                }
            } catch (Throwable e) {
                if (!fabricErrorLogged) {
                    fabricErrorLogged = true;
                    System.err.println("[BusAgent:fabric] 玩家监控首次异常：");
                    e.printStackTrace(System.err);
                }
            }
            try { Thread.sleep(1000); } catch (InterruptedException e) { break; }
        }
    }

    static void handleNeoForgeSync(String json) {
        if (json == null) {
            return;
        }
        if (nfMcInstance == null || nfMcClass == null) {
            if (!nfDiagInstanceLogged) {
                nfDiagInstanceLogged = true;
                System.err.println("[BusAgent:neoforge] 收到坐标，但 Minecraft 实例或类尚未就绪。");
            }
            return;
        }

        final double x;
        final double y;
        final double z;

        try {
            x = readJsonNumber(json, "x");
            y = readJsonNumber(json, "y");
            z = readJsonNumber(json, "z");
        } catch (Throwable e) {
            System.err.println("[BusAgent:neoforge] Invalid player_move message: " + e);
            return;
        }

        final Object mc = nfMcInstance;

        try {
            if (!nfDiagScheduleLogged) {
                nfDiagScheduleLogged = true;
                System.out.println("[BusAgent:neoforge] 已收到坐标，正在向 Minecraft 主线程提交同步任务。");
            }
            nfMcClass.getMethod("execute", Runnable.class).invoke(
                mc, (Runnable) () -> {
                    try {
                        ClassLoader loader = nfMcClass.getClassLoader();
                        Object level = nfMcClass.getField("level").get(mc);

                        if (level == null) {


                            if (!nfDiagLevelLogged) {


                                nfDiagLevelLogged = true;


                                System.err.println("[BusAgent:neoforge] 同步任务已执行，但 level=null；请确认已进入世界。");


                            }


                            return;


                        }


                        if (!nfDiagWorldReadyLogged) {


                            nfDiagWorldReadyLogged = true;


                            System.out.println("[BusAgent:neoforge] 世界已就绪，level=" + level.getClass().getName());


                        }

                        Class<?> levelClass = Class.forName(
                            "net.minecraft.client.multiplayer.ClientLevel", true, loader);
                        Class<?> playerClass = Class.forName(
                            "net.minecraft.client.player.RemotePlayer", true, loader);
                        Class<?> profileClass = Class.forName(
                            "com.mojang.authlib.GameProfile", true, loader);
                        Class<?> entityClass = Class.forName(
                            "net.minecraft.world.entity.Entity", true, loader);

                        // Remove the mirror from the previous world, if necessary.
                        if (remotePlayer != null && remotePlayerLevel != level) {
                            removeRemotePlayer(remotePlayerLevel, remotePlayer, loader);
                            remotePlayer = null;
                            remotePlayerLevel = null;
                        }

                        if (remotePlayer == null) {
                            Object profile = profileClass
                                .getConstructor(java.util.UUID.class, String.class)
                                .newInstance(remotePlayerUuid, "FabricMirror");

                            Object player = playerClass
                                .getConstructor(levelClass, profileClass)
                                .newInstance(level, profile);

                            Method getEntity = levelClass.getMethod("getEntity", int.class);
                            Method setId = entityClass.getMethod("setId", int.class);
                            Method setUUID = entityClass.getMethod(
                                "setUUID", java.util.UUID.class);

                            int candidate = 1_900_000_000;
                            boolean found = false;

                            for (int i = 0; i < 10000; i++, candidate--) {
                                if (getEntity.invoke(level, candidate) == null) {
                                    found = true;
                                    break;
                                }
                            }

                            if (!found) {
                                throw new IllegalStateException(
                                    "Could not find an unused entity ID");
                            }

                            setId.invoke(player, candidate);
                            setUUID.invoke(player, remotePlayerUuid);

                            entityClass.getMethod(
                                "setPos", double.class, double.class, double.class)
                                .invoke(player, x, y, z);
                            entityClass.getMethod("setOldPosAndRot").invoke(player);

                            levelClass.getMethod("addEntity", entityClass)
                                .invoke(level, player);

                            remotePlayer = player;
                            remotePlayerLevel = level;

                            System.out.println(String.format(
                                "[BusAgent:neoforge] RemotePlayer created: "
                                + "id=%d pos=(%.2f, %.2f, %.2f)",
                                candidate, x, y, z));
                        } else {
                            entityClass.getMethod(
                                "setPos", double.class, double.class, double.class)
                                .invoke(remotePlayer, x, y, z);
                            entityClass.getMethod("setOldPosAndRot")
                                .invoke(remotePlayer);
                        }
                    } catch (Throwable e) {
                        Throwable cause =
                            e instanceof java.lang.reflect.InvocationTargetException
                            && ((java.lang.reflect.InvocationTargetException)e).getCause() != null
                            ? ((java.lang.reflect.InvocationTargetException)e).getCause()
                            : e;
                        System.err.println(
                            "[BusAgent:neoforge] RemotePlayer update failed:");
                        cause.printStackTrace(System.err);
                    }
                }
            );
        } catch (Throwable e) {
            System.err.println(
                "[BusAgent:neoforge] Could not schedule RemotePlayer update:");
            e.printStackTrace(System.err);
        }
    }

    static double readJsonNumber(String json, String key) {
        java.util.regex.Pattern pattern = java.util.regex.Pattern.compile(
            "\"" + java.util.regex.Pattern.quote(key)
            + "\"\\s*:\\s*(-?(?:\\d+(?:\\.\\d*)?|\\.\\d+)(?:[eE][+-]?\\d+)?)"
        );

        java.util.regex.Matcher matcher = pattern.matcher(json);
        if (!matcher.find()) {
            throw new IllegalArgumentException("Missing numeric JSON field: " + key);
        }
        return Double.parseDouble(matcher.group(1));
    }

    static void removeRemotePlayer(Object level, Object player, ClassLoader loader) {
        if (level == null || player == null) {
            return;
        }

        try {
            Class<?> entityClass = Class.forName(
                "net.minecraft.world.entity.Entity", true, loader);
            Class<?> reasonClass = Class.forName(
                "net.minecraft.world.entity.Entity$RemovalReason", true, loader);

            @SuppressWarnings({"rawtypes", "unchecked"})
            Object discarded = Enum.valueOf(
                (Class<? extends Enum>) reasonClass, "DISCARDED");

            int id = ((Number) entityClass.getMethod("getId").invoke(player)).intValue();
            level.getClass().getMethod("removeEntity", int.class, reasonClass)
                .invoke(level, id, discarded);
        } catch (Throwable e) {
            System.err.println("[BusAgent:neoforge] Could not remove old mirror:");
            e.printStackTrace(System.err);
        }
    }

    static void bindXYZ(Class<?> clazz) {
        // 找 getX, getY, getZ 方法（包括继承来的）
        Class<?> c = clazz;
        while (c != null && (getXMethod == null || getYMethod == null || getZMethod == null)) {
            for (Method m : c.getDeclaredMethods()) {
                if (m.getParameterCount() == 0 && m.getReturnType() == double.class) {
                    if (m.getName().equals("getX") || m.getName().equals("dA")) getXMethod = m;
                    if (m.getName().equals("getY") || m.getName().equals("dC")) getYMethod = m;
                    if (m.getName().equals("getZ") || m.getName().equals("dG")) getZMethod = m;
                }
            }
            c = c.getSuperclass();
        }
        if (getXMethod != null) getXMethod.setAccessible(true);
        if (getYMethod != null) getYMethod.setAccessible(true);
        if (getZMethod != null) getZMethod.setAccessible(true);

        System.out.println("[BusAgent:" + myName + "] 坐标方法绑定：X="
            + (getXMethod == null ? "未找到" : getXMethod.getDeclaringClass().getName()
                + "#" + getXMethod.getName())
            + ", Y="
            + (getYMethod == null ? "未找到" : getYMethod.getDeclaringClass().getName()
                + "#" + getYMethod.getName())
            + ", Z="
            + (getZMethod == null ? "未找到" : getZMethod.getDeclaringClass().getName()
                + "#" + getZMethod.getName()));
    }
}
