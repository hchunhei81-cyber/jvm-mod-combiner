import java.io.*;
import java.net.*;
import java.lang.instrument.*;
import java.lang.reflect.*;

public class BusAgent {
    static PrintWriter out;
    static String myName;
    static Instrumentation inst;
    
    static Object mcInstance = null;
    static Field playerField = null;
    static Method getX, getY, getZ;

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        inst = instrumentation;
        myName = agentArgs != null ? agentArgs.toLowerCase().replace("-sandbox", "") : "unknown";
        System.out.println("[BusAgent:" + myName + "] premain");

        Thread t = new Thread(BusAgent::runBus);
        t.setDaemon(true);
        t.start();

        if (myName.equals("fabric")) {
            Thread monitor = new Thread(BusAgent::monitorPlayerActions);
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
                    System.out.println("[BusAgent:" + myName + "] 收到对方坐标: " + line);
                }
            }
        } catch (Exception e) {}
    }

    static void monitorPlayerActions() {
        System.out.println("[BusAgent:fabric] 启动坐标监控线程...");
        while (true) {
            try {
                if (mcInstance == null) {
                    try {
                        Class<?> loaderClass = Class.forName("net.fabricmc.loader.api.FabricLoader");
                        Object loader = loaderClass.getMethod("getInstance").invoke(null);
                        mcInstance = loaderClass.getMethod("getGameInstance").invoke(loader);
                        if (mcInstance != null) {
                            System.out.println("[BusAgent:fabric] ✅ 锁定游戏主类: " + mcInstance.getClass().getName());
                        }
                    } catch (Throwable ignored) {}
                }

                if (mcInstance != null && playerField == null) {
                    // 遍历主类的所有字段
                    for (Field f : mcInstance.getClass().getDeclaredFields()) {
                        f.setAccessible(true);
                        try {
                            Object val = f.get(mcInstance);
                            if (val == null) continue;

                            // 检查这个对象是不是玩家（特征是：有至少3个返回 double 的无参方法）
                            int doubleCount = 0;
                            Method[] methods = val.getClass().getMethods();
                            for (Method m : methods) {
                                if (m.getReturnType() == double.class && m.getParameterCount() == 0) {
                                    if (getX == null) getX = m;
                                    else if (getY == null) getY = m;
                                    else if (getZ == null) getZ = m;
                                    doubleCount++;
                                    if (doubleCount == 3) break; // 找到了三个坐标方法
                                }
                            }

                            if (doubleCount >= 3) {
                                playerField = f;
                                System.out.println("[BusAgent:fabric] ✅ 锁定实体/玩家字段: " + f.getName() + " 类型: " + val.getClass().getName());
                                break;
                            } else {
                                // 重置，因为可能只是普通的 double 方法
                                getX = null; getY = null; getZ = null;
                            }
                        } catch (Throwable ignored) {}
                    }
                }

                if (playerField != null && getX != null && getY != null && getZ != null) {
                    Object player = playerField.get(mcInstance);
                    if (player != null) {
                        double x = (double) getX.invoke(player);
                        double y = (double) getY.invoke(player);
                        double z = (double) getZ.invoke(player);
                        
                        String msg = String.format("{\"broadcast\":true,\"type\":\"player_move\",\"source\":\"fabric\",\"payload\":{\"x\":%.1f,\"y\":%.1f,\"z\":%.1f}}", x, y, z);
                        if (out != null) out.println(msg);
                    }
                }
            } catch (Throwable e) {
                mcInstance = null;
                playerField = null;
                getX = null; getY = null; getZ = null;
            }
            try { Thread.sleep(1000); } catch (InterruptedException e) { break; }
        }
    }
}
