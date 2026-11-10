import java.util.Locale;

public final class LoaderDetector {
    private LoaderDetector() {}

    public static String resolve(String systemProperty, String agentArgs) {
        if (systemProperty != null && !systemProperty.isBlank()) {
            return normalize(systemProperty);
        }

        return normalize(agentArgs);
    }

    public static String normalize(String value) {
        if (value == null || value.isBlank()) {
            return "unknown";
        }

        String normalized = value
            .trim()
            .toLowerCase(Locale.ROOT)
            .replace("-sandbox", "");

        return switch (normalized) {
            case "fabric" -> "fabric";
            case "forge" -> "forge";
            case "neoforge" -> "neoforge";
            default -> "unknown";
        };
    }
}
