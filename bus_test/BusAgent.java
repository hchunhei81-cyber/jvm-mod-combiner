import java.lang.instrument.Instrumentation;
import java.io.*;
import java.net.*;

public class BusAgent {
    public static void main(String[] args) throws Exception {
        // 允许手动跑: java BusAgent fabric
        run(args.length > 0 ? args[0] : "manual");
        Thread.sleep(3000);
    }

    public static void premain(String agentArgs, Instrumentation inst) {
        String sandboxName = agentArgs != null ? agentArgs : "Unknown-Sandbox";
        Thread t = new Thread(() -> run(sandboxName));
        t.setDaemon(true);
        t.start();
    }

    static void run(String sandboxName) {
        // 名字归一化: Fabric-Sandbox -> fabric, NeoForge-Sandbox -> neoforge
        String name = sandboxName.toLowerCase().contains("neoforge") ? "neoforge"
                    : sandboxName.toLowerCase().contains("fabric") ? "fabric"
                    : sandboxName.toLowerCase();
        String peer = name.equals("fabric") ? "neoforge" : "fabric";

        try (Socket socket = new Socket("127.0.0.1", 25565);
             BufferedReader in = new BufferedReader(new InputStreamReader(socket.getInputStream()));
             PrintWriter out = new PrintWriter(socket.getOutputStream(), true)) {

            // 1. 注册
            out.println("{\"register\":\"" + name + "\"}");
            System.out.println("[BusAgent:" + name + "] 已注册并连入总线");

            // 2. 发一条给对面
            out.println("{\"to\":\"" + peer + "\",\"type\":\"hello\",\"payload\":\"hi from " + name + "\"}");
            System.out.println("[BusAgent:" + name + "] 已发送 hello -> " + peer);

            // 3. 持续读
            String line;
            while ((line = in.readLine()) != null) {
                System.out.println("[BusAgent:" + name + "] 收到: " + line);
                if (line.contains("\"routed\":true")) {
                    System.out.println("[BusAgent:" + name + "] 路由回执确认");
                }
                if (line.contains("\"type\":\"hello\"") && line.contains("\"from\":\"" + peer + "\"")) {
                    System.out.println("[BusAgent:" + name + "] ✅ 收到 " + peer + " 的跨沙盒消息，回 ack");
                    out.println("{\"to\":\"" + peer + "\",\"type\":\"ack\",\"payload\":\"ack from " + name + "\"}");
                }
            }
        } catch (Exception e) {
            System.out.println("[BusAgent:" + name + "] 连接失败: " + e.getMessage());
        }
    }
}