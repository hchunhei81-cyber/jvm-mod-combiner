import java.io.*;
import java.net.*;
import java.lang.instrument.*;
import java.security.ProtectionDomain;

public class BusAgent {
    public static void premain(String agentArgs, Instrumentation inst) {
        String name = agentArgs != null ? agentArgs : "unknown";
        final String tag = name.toLowerCase().contains("neoforge") ? "neoforge"
                         : name.toLowerCase().contains("fabric") ? "fabric"
                         : name;

        System.out.println("[BusAgent:" + tag + "] premain");

        // 1. Instrumentation Transformer：精确拦截主类
        inst.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(ClassLoader loader, String className,
                                    Class<?> classBeingRedefined,
                                    ProtectionDomain protectionDomain,
                                    byte[] classfileBuffer) {
                if (className == null) return null;
                if (className.startsWith("net/minecraft/client/") || className.contains("class_310")) {
                    System.out.println("[BusAgent:" + tag + "] MC-CLIENT: " + className
                        + " loader=" + (loader == null ? "bootstrap" : loader.getClass().getSimpleName()));
                }
                return null;
            }
        });

        // 2. 定时反射探测：确保在启动后期能抓到
        Thread probe = new Thread(() -> {
            String[] candidates = {
                "net.minecraft.client.MinecraftClient",
                "net.minecraft.client.Minecraft",
                "net.minecraft.class_310"
            };
            for (int i = 0; i < 120; i++) {
                for (String c : candidates) {
                    try {
                        Class<?> mc = Class.forName(c);
                        System.out.println("[BusAgent:" + tag + "] OK 探测到 " + c
                            + " loader=" + mc.getClassLoader().getClass().getSimpleName());
                        return;
                    } catch (ClassNotFoundException ignored) {}
                }
                try { Thread.sleep(1000); } catch (InterruptedException e) { return; }
            }
            System.out.println("[BusAgent:" + tag + "] FAIL 120秒内没探测到主类");
        });
        probe.setDaemon(true);
        probe.start();

        // 3. 总线
        Thread bus = new Thread(() -> {
            try (Socket s = new Socket("127.0.0.1", 25565);
                 PrintWriter out = new PrintWriter(s.getOutputStream(), true);
                 BufferedReader in = new BufferedReader(new InputStreamReader(s.getInputStream()))) {
                out.println("{\"register\":\"" + tag + "\"}");
                String line;
                while ((line = in.readLine()) != null) {
                    System.out.println("[BusAgent:" + tag + "] 收到: " + line);
                }
            } catch (Exception e) {
                System.out.println("[BusAgent:" + tag + "] 总线连接失败(可忽略): " + e.getMessage());
            }
        });
        bus.setDaemon(true);
        bus.start();
    }
}
