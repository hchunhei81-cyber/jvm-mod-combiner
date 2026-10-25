import java.io.*;
import java.net.*;

public class SandboxClient {
    public static void main(String[] args) {
        String sandboxName = args.length > 0 ? args[0] : "Unknown";
        try (Socket socket = new Socket("127.0.0.1", 25565);
             PrintWriter out = new PrintWriter(socket.getOutputStream(), true)) {
            out.println("Hello from " + sandboxName);
            Thread.sleep(2000); // 保持连接一会儿防止瞬间断开
        } catch (Exception e) {
            System.out.println("[" + sandboxName + "] 连接失败: " + e.getMessage());
        }
    }
}
