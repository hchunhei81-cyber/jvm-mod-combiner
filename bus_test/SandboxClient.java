import java.io.IOException;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.channels.SocketChannel;
import java.nio.file.Path;

public final class SandboxClient {
    public static void main(String[] args) {
        String probeName = args.length > 0 ? args[0] : "UDS-Probe";
        String socketPath = System.getenv("JVM_COMBINER_BUS_SOCKET");

        if (socketPath == null || socketPath.isBlank()) {
            System.err.println("[" + probeName + "] JVM_COMBINER_BUS_SOCKET is missing");
            System.exit(2);
        }

        try (SocketChannel channel =
                     SocketChannel.open(StandardProtocolFamily.UNIX)) {
            channel.connect(UnixDomainSocketAddress.of(Path.of(socketPath)));
            System.out.println("[" + probeName
                    + "] UDS transport connected; probe is not Loader-authorized");
        } catch (IOException | RuntimeException error) {
            System.err.println("[" + probeName + "] UDS connection failed: "
                    + error.getMessage());
            System.exit(1);
        }
    }
}
