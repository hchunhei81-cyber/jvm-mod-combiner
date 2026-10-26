import java.io.*;
import java.net.*;
import java.lang.instrument.*;
import java.lang.reflect.*;

public class BusAgent {
    static PrintWriter out;
    static String myName;
    static Instrumentation inst;

    static Object fabricMcInstance = null;
    static Field fabricPlayerField = null;
    static Method fGetX, fGetY, fGetZ;

    static volatile Object nfMcInstance = null;
    static volatile Class<?> nfMcClass = null;

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
                                Thread.sleep(3000);
                                nfMcClass = Class.forName("net.minecraft.client.Minecraft", false, targetLoader);
                                nfMcInstance = nfMcClass.getMethod("getInstance").invoke(null);
                                System.out.println("[BusAgent:neoforge] ✅ 成功锁定 Minecraft 实例！");
                            } catch (Throwable e) {
                                System.out.println("[BusAgent:neoforge] ❌ 获取实例失败: " + e);
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
        while (true) {
            try {
                if (fabricMcInstance == null) {
                    Class<?> loaderClass = Class.forName("net.fabricmc.loader.api.FabricLoader");
                    Object loader = loaderClass.getMethod("getInstance").invoke(null);
                    fabricMcInstance = loaderClass.getMethod("getGameInstance").invoke(loader);
                }

                if (fabricMcInstance != null && fabricPlayerField == null) {
                    for (Field f : fabricMcInstance.getClass().getDeclaredFields()) {
                        f.setAccessible(true);
                        Object val = f.get(fabricMcInstance);
                        if (val != null && countDoubleMethods(val.getClass()) >= 3) {
                            fabricPlayerField = f;
                            break;
                        }
                    }
                }

                if (fabricPlayerField != null) {
                    Object player = fabricPlayerField.get(fabricMcInstance);
                    if (player != null) {
                        if (fGetX == null) bindXYZ(player.getClass());
                        double x = (double) fGetX.invoke(player);
                        double y = (double) fGetY.invoke(player);
                        double z = (double) fGetZ.invoke(player);
                        String msg = String.format("{\"broadcast\":true,\"type\":\"player_move\",\"source\":\"fabric\",\"payload\":{\"x\":%.1f,\"y\":%.1f,\"z\":%.1f}}", x, y, z);
                        if (out != null) out.println(msg);
                    }
                }
            } catch (Throwable ignored) {}
            try { Thread.sleep(1000); } catch (InterruptedException e) { break; }
        }
    }

    static void handleNeoForgeSync(String json) {
        try {
            if (nfMcInstance == null) {
                if (nfMcClass != null) nfMcInstance = nfMcClass.getMethod("getInstance").invoke(null);
                else return;
            }

            double x = 0, y = 0, z = 0;
            int xIdx = json.indexOf("\"x\":");
            int yIdx = json.indexOf("\"y\":");
            int zIdx = json.indexOf("\"z\":");
            if (xIdx != -1 && yIdx != -1 && zIdx != -1) {
                x = Double.parseDouble(json.substring(xIdx + 4, json.indexOf(",", xIdx)));
                y = Double.parseDouble(json.substring(yIdx + 4, json.indexOf(",", yIdx)));
                z = Double.parseDouble(json.substring(zIdx + 4, json.indexOf("}", zIdx)));
            }

            // 尝试在 NeoForge 的 GUI 里直接刷一条聊天消息
            String msg = String.format("[Fusion] Fabric 玩家坐标: %.1f, %.1f, %.1f", x, y, z);
            if (addChatMessage(nfMcInstance, msg)) {
                System.out.println("[BusAgent:neoforge] 🎯 收到 Fabric 坐标，并在游戏界面打印: " + msg);
            } else {
                System.out.println("[BusAgent:neoforge] ⚠️ 收到坐标，但无法在界面打印: " + msg);
            }

        } catch (Throwable e) {
            System.out.println("[BusAgent:neoforge] 同步失败: " + e);
        }
    }

    // 在 Minecraft 的聊天界面里输出消息
    static boolean addChatMessage(Object mcInstance, String msg) {
        if (mcInstance == null) return false;
        
        Object inGameHud = null;
        for (Field f : mcInstance.getClass().getDeclaredFields()) {
            f.setAccessible(true);
            try {
                Object val = f.get(mcInstance);
                if (val != null && val.getClass().getName().contains("InGameHud")) {
                    inGameHud = val;
                    break;
                }
            } catch (Throwable ignored) {}
        }
        if (inGameHud == null) return false;

        Object chatHud = null;
        for (Field f : inGameHud.getClass().getDeclaredFields()) {
            f.setAccessible(true);
            try {
                Object val = f.get(inGameHud);
                if (val != null && val.getClass().getName().contains("ChatHud")) {
                    chatHud = val;
                    break;
                }
            } catch (Throwable ignored) {}
        }
        if (chatHud == null) return false;

        try {
            Method addMessage = chatHud.getClass().getMethod("addMessage", String.class);
            addMessage.invoke(chatHud, msg);
            return true;
        } catch (Throwable e) {
            return false;
        }
    }

    static int countDoubleMethods(Class<?> clazz) {
        int count = 0;
        for (Method m : clazz.getMethods()) {
            if (m.getReturnType() == double.class && m.getParameterCount() == 0) {
                count++;
                if (count >= 3) return count;
            }
        }
        return count;
    }

    static void bindXYZ(Class<?> clazz) {
        for (Method m : clazz.getMethods()) {
            if (m.getReturnType() == double.class && m.getParameterCount() == 0) {
                if (fGetX == null) fGetX = m;
                else if (fGetY == null) fGetY = m;
                else if (fGetZ == null) fGetZ = m;
            }
        }
    }
}
