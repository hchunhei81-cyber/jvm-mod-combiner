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
    static volatile boolean fabricDiagnosticPrinted = false;
    static Method getXMethod, getYMethod, getZMethod;
    static Method getYRotMethod, getXRotMethod, isSprintingMethod;

    // NeoForge
    static volatile Object nfMcInstance = null;
    static volatile Class<?> nfMcClass = null;
    static volatile Object remotePlayer = null;
    static volatile Object remotePlayerLevel = null;

    static final java.util.UUID remotePlayerUuid = java.util.UUID.nameUUIDFromBytes(
        "jvm-mod-combiner:fabric-mirror".getBytes(java.nio.charset.StandardCharsets.UTF_8));

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        inst = instrumentation;
        myName = agentArgs != null ? agentArgs.toLowerCase().replace("-sandbox", "") : "unknown";
        System.out.println("[BusAgent:" + myName + "] premain");

        if (myName.equals("neoforge")) {
            inst.addTransformer(new ClassFileTransformer() {
                @Override
                public byte[] transform(ClassLoader loader, String className,
                                        Class<?> classBeingRedefined,
                                        java.security.ProtectionDomain protectionDomain,
                                        byte[] classfileBuffer) {
                    if ("net/minecraft/client/Minecraft".equals(className)) {
                        System.out.println(
                            "[BusAgent:neoforge] 检测到 Minecraft 类加载，loader=" + loader);
                    }
                    return null;
                }
            });

            Thread instanceFinder = new Thread(() -> {
                long deadline = System.currentTimeMillis() + 120000;
                Throwable lastError = null;

                while (System.currentTimeMillis() < deadline
                        && nfMcInstance == null) {
                    try {
                        ClassLoader contextLoader =
                            Thread.currentThread().getContextClassLoader();

                        Class<?> mcClass = null;
                        ClassLoader[] candidates = new ClassLoader[] {
                            contextLoader,
                            BusAgent.class.getClassLoader()
                        };

                        for (ClassLoader candidate : candidates) {
                            if (candidate == null) continue;
                            try {
                                mcClass = Class.forName(
                                    "net.minecraft.client.Minecraft",
                                    false, candidate);
                                break;
                            } catch (ClassNotFoundException ignored) {
                            }
                        }

                        if (mcClass == null) {
                            lastError = new ClassNotFoundException(
                                "候选 ClassLoader 均无法找到 Minecraft 类");
                        } else {
                            Object instance = mcClass.getMethod("getInstance")
                                .invoke(null);

                            if (instance != null) {
                                nfMcClass = mcClass;
                                nfMcInstance = instance;
                                System.out.println(
                                    "[BusAgent:neoforge] 成功获取 Minecraft 实例！"
                                    + " loader=" + mcClass.getClassLoader());
                                return;
                            }
                        }
                    } catch (Throwable e) {
                        lastError = e;
                    }

                    try {
                        Thread.sleep(500);
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        return;
                    }
                }

                if (nfMcInstance == null) {
                    System.err.println(
                        "[BusAgent:neoforge] 等待 120 秒仍未获取 Minecraft 实例。");
                    if (lastError != null) lastError.printStackTrace();
                }
            }, "BusAgent-MinecraftInstanceFinder");

            instanceFinder.setDaemon(true);
            instanceFinder.start();
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
        } catch (Exception e) {}
    }



    static void monitorFabricPlayer() {
        Class<?> clientClass = null;
        Field clientInstanceField = null;

        while (true) {
            try {
                // 不使用 Class.forName("flk")，避免 AppClassLoader 重复加载。
                if (clientClass == null) {
                    if (inst != null) {
                        for (Class<?> c : inst.getAllLoadedClasses()) {
                            if (c.getName().equals("flk")) {
                                clientClass = c;
                                System.out.println(
                                    "[BusAgent:fabric] 找到已加载的 Minecraft 类: "
                                    + c.getName()
                                    + " | ClassLoader="
                                    + c.getClassLoader()
                                );
                                break;
                            }
                        }
                    }
                }

                // 通过已经加载的 Class 对象访问静态字段 F。
                if (clientClass != null && clientInstanceField == null) {
                    clientInstanceField = clientClass.getDeclaredField("F");
                    clientInstanceField.setAccessible(true);
                }

                if (clientInstanceField != null && fabricMcInstance == null) {
                    fabricMcInstance = clientInstanceField.get(null);

                    if (fabricMcInstance != null) {
                        System.out.println(
                            "[BusAgent:fabric] 成功锁定 Minecraft 实例: "
                            + fabricMcInstance.getClass().getName()
                        );
                    }
                }

                if (fabricMcInstance != null && fabricPlayerField == null) {
                    fabricPlayerField =
                        fabricMcInstance.getClass().getDeclaredField("t");
                    fabricPlayerField.setAccessible(true);

                    System.out.println(
                        "[BusAgent:fabric] 已绑定 LocalPlayer 字段: t"
                    );
                }

                if (fabricPlayerField != null) {
                    Object player = fabricPlayerField.get(fabricMcInstance);

                    if (player != null) {
                        if (getXMethod == null) {
                            bindMethods(player.getClass());
                        }

                        if (getXMethod != null &&
                            getYMethod != null &&
                            getZMethod != null) {

                            double x = (double) getXMethod.invoke(player);
                            double y = (double) getYMethod.invoke(player);
                            double z = (double) getZMethod.invoke(player);

                            float yaw = getYRotMethod != null
                                ? (float) getYRotMethod.invoke(player) : 0f;

                            float pitch = getXRotMethod != null
                                ? (float) getXRotMethod.invoke(player) : 0f;

                            boolean sprinting = isSprintingMethod != null
                                && (boolean) isSprintingMethod.invoke(player);

                            String msg = String.format(
                                Locale.US,
                                "{\"broadcast\":true,\"type\":\"player_move\",\"source\":\"fabric\",\"payload\":{\"x\":%.3f,\"y\":%.3f,\"z\":%.3f,\"yaw\":%.3f,\"pitch\":%.3f,\"sprint\":%b}}",
                                x, y, z, yaw, pitch, sprinting
                            );

                            PrintWriter writer = out;
                            if (writer != null) {
                                writer.println(msg);
                            }
                        }
                    }
                }

            } catch (Throwable e) {
                if (!fabricDiagnosticPrinted) {
                    fabricDiagnosticPrinted = true;
                    System.out.println(
                        "[BusAgent:fabric] 首次同步异常: "
                        + e.getClass().getName() + ": " + e.getMessage()
                    );
                    e.printStackTrace();
                }
            }

            try {
                Thread.sleep(100);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            }
        }
    }

    static void handleNeoForgeSync(String json) {
        System.out.println("[BusAgent:neoforge] 收到同步消息: " + json);

        try {
            if (nfMcInstance == null || nfMcClass == null) {
                System.out.println("[BusAgent:neoforge] Minecraft 实例尚未就绪");
                return;
            }

            double x = readJsonNumber(json, "x");
            double y = readJsonNumber(json, "y");
            double z = readJsonNumber(json, "z");
            float yaw = (float) readJsonNumber(json, "yaw");
            float pitch = (float) readJsonNumber(json, "pitch");
            boolean sprint = json.contains("\"sprint\":true");

            Object mc = nfMcInstance;
            Method execute = nfMcClass.getMethod("execute", Runnable.class);

            execute.invoke(mc, (Runnable) () -> {
                try {
                    Object level = nfMcClass.getField("level").get(mc);

                    if (level == null) {
                        System.out.println("[BusAgent:neoforge] 当前世界为空，等待进入世界");
                        return;
                    }

                    ClassLoader loader = nfMcClass.getClassLoader();

                    Class<?> levelClass = Class.forName(
                        "net.minecraft.client.multiplayer.ClientLevel", true, loader);
                    Class<?> playerClass = Class.forName(
                        "net.minecraft.client.player.RemotePlayer", true, loader);
                    Class<?> profileClass = Class.forName(
                        "com.mojang.authlib.GameProfile", true, loader);
                    Class<?> entityClass = Class.forName(
                        "net.minecraft.world.entity.Entity", true, loader);

                    if (remotePlayer != null && remotePlayerLevel != level) {
                        System.out.println("[BusAgent:neoforge] 世界已切换，重建镜像玩家");
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
                        boolean idAvailable = false;

                        for (int i = 0; i < 10000; i++) {
                            if (getEntity.invoke(level, candidate) == null) {
                                idAvailable = true;
                                break;
                            }
                            candidate--;
                        }

                        if (!idAvailable) {
                            throw new IllegalStateException(
                                "找不到可用的镜像玩家实体 ID");
                        }

                        setId.invoke(player, candidate);
                        setUUID.invoke(player, remotePlayerUuid);

                        entityClass.getMethod(
                            "setPos", double.class, double.class, double.class)
                            .invoke(player, x, y, z);

                        entityClass.getMethod("setYRot", float.class)
                            .invoke(player, yaw);
                        entityClass.getMethod("setXRot", float.class)
                            .invoke(player, pitch);

                        try {
                            entityClass.getMethod("setYHeadRot", float.class)
                                .invoke(player, yaw);
                        } catch (NoSuchMethodException ignored) {}

                        try {
                            entityClass.getMethod("setYBodyRot", float.class)
                                .invoke(player, yaw);
                        } catch (NoSuchMethodException ignored) {}

                        try {
                            entityClass.getMethod("setSprinting", boolean.class)
                                .invoke(player, sprint);
                        } catch (NoSuchMethodException ignored) {}

                        entityClass.getMethod("setOldPosAndRot").invoke(player);

                        System.out.println(
                            "[BusAgent:neoforge] 准备加入世界：ID=" + candidate
                            + "，位置=(" + x + ", " + y + ", " + z + ")");

                        levelClass.getMethod("addEntity", entityClass)
                            .invoke(level, player);

                        Object registered = getEntity.invoke(level, candidate);

                        if (registered == null) {
                            throw new IllegalStateException(
                                "addEntity 执行后，ClientLevel.getEntity(ID) 仍返回 null");
                        }

                        remotePlayer = player;
                        remotePlayerLevel = level;

                        System.out.println(
                            "[BusAgent:neoforge] 镜像玩家已加入世界："
                            + player.getClass().getName()
                            + "，ID=" + candidate
                            + "，UUID=" + remotePlayerUuid);

                    } else {
                        entityClass.getMethod(
                            "setPos", double.class, double.class, double.class)
                            .invoke(remotePlayer, x, y, z);

                        entityClass.getMethod("setYRot", float.class)
                            .invoke(remotePlayer, yaw);
                        entityClass.getMethod("setXRot", float.class)
                            .invoke(remotePlayer, pitch);

                        try {
                            entityClass.getMethod("setYHeadRot", float.class)
                                .invoke(remotePlayer, yaw);
                        } catch (NoSuchMethodException ignored) {}

                        try {
                            entityClass.getMethod("setYBodyRot", float.class)
                                .invoke(remotePlayer, yaw);
                        } catch (NoSuchMethodException ignored) {}

                        try {
                            entityClass.getMethod("setSprinting", boolean.class)
                                .invoke(remotePlayer, sprint);
                        } catch (NoSuchMethodException ignored) {}

                        entityClass.getMethod("setOldPosAndRot")
                            .invoke(remotePlayer);
                    }

                } catch (Throwable e) {
                    System.err.println(
                        "[BusAgent:neoforge] 主线程同步失败："
                        + e.getClass().getName() + ": " + e.getMessage());
                    e.printStackTrace();
                }
            });

        } catch (Throwable e) {
            System.err.println(
                "[BusAgent:neoforge] 消息处理失败："
                + e.getClass().getName() + ": " + e.getMessage());
            e.printStackTrace();
        }
    }

    static double readJsonNumber(String json, String key) {
        java.util.regex.Pattern p = java.util.regex.Pattern.compile(
            "\"" + key + "\"\\s*:\\s*(-?\\d+(?:\\.\\d+)?)");
        java.util.regex.Matcher m = p.matcher(json);
        if (m.find()) return Double.parseDouble(m.group(1));
        return 0;
    }

    static Method findNoArgMethod(Class<?> clazz, String name, Class<?> returnType) {
        Class<?> current = clazz;

        while (current != null) {
            try {
                Method method = current.getDeclaredMethod(name);

                if (method.getReturnType() == returnType) {
                    method.setAccessible(true);
                    return method;
                }
            } catch (NoSuchMethodException ignored) {
            } catch (Throwable e) {
                System.out.println(
                    "[BusAgent:fabric] 方法绑定异常 " + name + ": " + e
                );
            }

            current = current.getSuperclass();
        }

        return null;
    }

    static void bindMethods(Class<?> clazz) {
        System.out.println(
            "[BusAgent:fabric] 正在绑定 Minecraft 1.21.4 方法: "
            + clazz.getName()
        );

        getXMethod = findNoArgMethod(clazz, "dA", double.class);
        getYMethod = findNoArgMethod(clazz, "dC", double.class);
        getZMethod = findNoArgMethod(clazz, "dG", double.class);

        getYRotMethod = findNoArgMethod(clazz, "dL", float.class);
        getXRotMethod = findNoArgMethod(clazz, "dN", float.class);
        isSprintingMethod = findNoArgMethod(clazz, "cj", boolean.class);

        System.out.println(
            "[BusAgent:fabric] 绑定结果: "
            + "X=" + (getXMethod != null)
            + ", Y=" + (getYMethod != null)
            + ", Z=" + (getZMethod != null)
            + ", YRot=" + (getYRotMethod != null)
            + ", XRot=" + (getXRotMethod != null)
            + ", Sprint=" + (isSprintingMethod != null)
        );
    }
}
