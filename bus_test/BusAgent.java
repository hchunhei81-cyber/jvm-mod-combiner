import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.PrintWriter;
import java.lang.instrument.Instrumentation;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

public final class BusAgent {
    private static final String HOST = "127.0.0.1";
    private static final int PORT = 25565;

    private static boolean hasMethodInHierarchy(
            Class<?> type, String methodName) {

        Class<?> current = type;

        while (current != null) {
            try {
                current.getDeclaredMethod(methodName);
                return true;
            } catch (NoSuchMethodException ignored) {
                current = current.getSuperclass();
            } catch (Throwable ignored) {
                return false;
            }
        }

        return false;
    }

    private static volatile String myName = "unknown";
    private static volatile PrintWriter out;

    private static volatile Object minecraft;
    private static volatile Field playerField;

    private static final AtomicBoolean monitorStarted = new AtomicBoolean(false);

    private static final Pattern X_PATTERN = Pattern.compile("\"x\"\\s*:\\s*(-?\\d+(?:\\.\\d+)?(?:[eE][+-]?\\d+)?)");
    private static final Pattern Y_PATTERN = Pattern.compile("\"y\"\\s*:\\s*(-?\\d+(?:\\.\\d+)?(?:[eE][+-]?\\d+)?)");
    private static final Pattern Z_PATTERN = Pattern.compile("\"z\"\\s*:\\s*(-?\\d+(?:\\.\\d+)?(?:[eE][+-]?\\d+)?)");

    private static Object findFabricMinecraftInstance() {
        try {
            Class<?> loaderClass = Class.forName(
                    "net.fabricmc.loader.api.FabricLoader");

            Object loader = loaderClass
                    .getMethod("getInstance")
                    .invoke(null);

            Object instance = loaderClass
                    .getMethod("getGameInstance")
                    .invoke(loader);

            return instance;
        } catch (Throwable e) {
            log("Fabric game instance lookup failed: "
                    + e.getClass().getSimpleName()
                    + ": " + e.getMessage());

            return null;
        }
    }

    public static void premain(String args, Instrumentation instrumentation) {
        myName = args == null
                ? "unknown"
                : args.toLowerCase(Locale.ROOT).replace("-sandbox", "").trim();

        log("premain; agent name = " + myName);

        Thread busThread = new Thread(BusAgent::connectLoop, "BusAgent-TCP");
        busThread.setDaemon(true);
        busThread.start();

        if ("fabric".equals(myName)) {
            startMonitor(instrumentation);
        } else if ("neoforge".equals(myName)) {
            startMonitor(instrumentation);
        } else {
            log("WARNING: unexpected agent argument: " + myName);
        }
    }

    private static void log(String message) {
        System.out.println("[BusAgent:" + myName + "] " + message);
    }

    private static void startMonitor(Instrumentation instrumentation) {
        if (!monitorStarted.compareAndSet(false, true)) {
            log("Monitor already started in this JVM.");
            return;
        }

        Thread thread = new Thread(
                () -> monitorGame(instrumentation),
                "BusAgent-GameMonitor");
        thread.setDaemon(true);
        thread.start();
    }

    private static void connectLoop() {
        while (!Thread.currentThread().isInterrupted()) {
            try (Socket socket = new Socket()) {
                socket.connect(new InetSocketAddress(HOST, PORT), 3000);
                socket.setKeepAlive(true);
                socket.setTcpNoDelay(true);

                try (
                        BufferedReader in = new BufferedReader(
                                new InputStreamReader(
                                        socket.getInputStream(),
                                        StandardCharsets.UTF_8));
                        PrintWriter writer = new PrintWriter(
                                socket.getOutputStream(),
                                true,
                                StandardCharsets.UTF_8)) {
                    out = writer;
                    writer.println("{\"register\":\"" + myName + "\"}");
                    log("Connected to data bus at " + HOST + ":" + PORT);

                    String line;
                    while ((line = in.readLine()) != null) {
                        handleMessage(line);
                    }

                    log("Data bus connection closed.");
                }
            } catch (Exception e) {
                out = null;
                log("Data bus connection failed: " + e.getMessage());
            }

            out = null;

            try {
                Thread.sleep(2000);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            }
        }
    }

    private static void send(String json) {
        PrintWriter writer = out;

        if (writer == null) {
            return;
        }

        synchronized (writer) {
            writer.println(json);

            if (writer.checkError()) {
                log("WARNING: failed to write to data bus.");
                out = null;
            }
        }
    }

    private static void handleMessage(String json) {
        if (!json.contains("\"type\":\"player_move\"")
                && !json.contains("\"type\": \"player_move\"")) {
            return;
        }

        if ("neoforge".equals(myName)) {
            Double x = extract(X_PATTERN, json);
            Double y = extract(Y_PATTERN, json);
            Double z = extract(Z_PATTERN, json);

            if (x != null && y != null && z != null) {
                log(String.format(
                        Locale.US,
                        "[FUSION-SYNC] Fabric Player Pos: %.2f, %.2f, %.2f",
                        x, y, z));
            } else {
                log("Received player_move but could not parse coordinates: " + json);
            }
        }
    }

    private static Double extract(Pattern pattern, String json) {
        Matcher matcher = pattern.matcher(json);
        if (!matcher.find()) {
            return null;
        }

        try {
            return Double.parseDouble(matcher.group(1));
        } catch (NumberFormatException e) {
            return null;
        }
    }

    private static void monitorGame(Instrumentation instrumentation) {
        boolean reportedMissingMinecraft = false;
        boolean reportedMissingPlayer = false;
        boolean reportedMissingGetters = false;

        while (!Thread.currentThread().isInterrupted()) {
            try {
                if (minecraft == null) {
                    if ("fabric".equals(myName)) {
                        // Fabric: obtain the game instance through Fabric Loader.
                        minecraft = findFabricMinecraftInstance();
                    } else {
                        // NeoForge: obtain the loaded Minecraft class through Instrumentation.
                        Class<?> mcClass = findMinecraftClass(instrumentation);

                        if (mcClass != null) {
                            try {
                                Method getInstance = mcClass.getDeclaredMethod("getInstance");

                                getInstance.setAccessible(true);
                                minecraft = getInstance.invoke(null);
                            } catch (Throwable e) {
                                log("Minecraft getInstance() failed: " + e);
                            }
                        }
                    }

                    if (minecraft != null) {
                        log("Minecraft instance acquired: "
                                + minecraft.getClass().getName());
                        reportedMissingMinecraft = false;
                    } else if (!reportedMissingMinecraft) {
                        log("Waiting for Minecraft game instance...");
                        reportedMissingMinecraft = true;
                    }
                }
                if (minecraft != null && "fabric".equals(myName)) {
                    Object player = getPlayer(minecraft);

                    if (player == null) {
                        if (!reportedMissingPlayer) {
                            log("Waiting for the player to enter a world.");
                            reportedMissingPlayer = true;
                        }
                    } else {
                        reportedMissingPlayer = false;

                        Double x = invokeCoordinate(player, "getX");
                        Double y = invokeCoordinate(player, "getY");
                        Double z = invokeCoordinate(player, "getZ");

                        if (x != null && y != null && z != null) {
                            reportedMissingGetters = false;

                            String message = String.format(
                                    Locale.US,
                                    "{\"broadcast\":true,\"type\":\"player_move\","
                                            + "\"source\":\"fabric\","
                                            + "\"payload\":{\"x\":%.4f,\"y\":%.4f,\"z\":%.4f}}",
                                    x, y, z);

                            send(message);

                            log(String.format(
                                    Locale.US,
                                    "Player coordinates: %.2f, %.2f, %.2f",
                                    x, y, z));
                        } else if (!reportedMissingGetters) {
                            log("Could not call getX/getY/getZ on player class "
                                    + player.getClass().getName());
                            logCoordinateDiagnostics(player);
                            reportedMissingGetters = true;
                        }
                    }
                }
            } catch (Throwable e) {
                log("Monitor error: " + e.getClass().getSimpleName()
                        + ": " + e.getMessage());
            }

            try {
                Thread.sleep(1000);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            }
        }
    }

    private static Class<?> findMinecraftClass(Instrumentation instrumentation) {
        for (Class<?> clazz : instrumentation.getAllLoadedClasses()) {
            if ("net.minecraft.client.Minecraft".equals(clazz.getName())) {
                return clazz;
            }
        }

        return null;
    }

    private static Object getPlayer(Object mc) {
        // Minecraft 1.21.4: flk.t is the gkx player field.
        // Prefer this known field before scanning other client fields.
        try {
            Field field = mc.getClass().getDeclaredField("t");
            field.setAccessible(true);

            Object value = field.get(mc);

            if (value != null
                    && hasMethodInHierarchy(value.getClass(), "dA")
                    && hasMethodInHierarchy(value.getClass(), "dC")
                    && hasMethodInHierarchy(value.getClass(), "dG")) {

                playerField = field;
                return value;
            }
        } catch (Throwable ignored) {
            // Fall back to the existing lookup if this field is unavailable.
        }
        try {
            Field cached = playerField;

            if (cached != null) {
                Object value = cached.get(mc);

                if (value != null) {
                    return value;
                }
            }

            Class<?> type = mc.getClass();

            while (type != null) {
                for (Field field : type.getDeclaredFields()) {
                    try {
                        field.setAccessible(true);

                        Object value = field.get(mc);

                        if (value == null) {
                            continue;
                        }

                        Class<?> valueClass = value.getClass();

                        if (hasCoordinateGetters(valueClass)
                                || isRecognizablePlayerType(valueClass)) {
                            playerField = field;

                            log("Player field identified: "
                                    + field.getName()
                                    + "; class = "
                                    + valueClass.getName());

                            return value;
                        }
                    } catch (Throwable ignored) {
                        // Skip inaccessible or unrelated fields.
                    }
                }

                type = type.getSuperclass();
            }

            logPlayerFieldDiagnostics(mc);
        } catch (Throwable e) {
            log("Player lookup failed: "
                    + e.getClass().getSimpleName()
                    + ": " + e.getMessage());
        }

        return null;
    }

    private static volatile long lastPlayerDiagnosticsAt = 0L;

    private static void logPlayerFieldDiagnostics(Object mc) {
        long now = System.currentTimeMillis();

        if (now - lastPlayerDiagnosticsAt < 10000L) {
            return;
        }

        lastPlayerDiagnosticsAt = now;

        log("===== Minecraft field diagnostics =====");
        log("Minecraft class: " + mc.getClass().getName());

        Class<?> type = mc.getClass();

        while (type != null) {
            log("Inspecting class: " + type.getName());

            for (Field field : type.getDeclaredFields()) {
                try {
                    field.setAccessible(true);

                    Object value = field.get(mc);

                    log("Field: " + field.getName()
                            + "; declaredType=" + field.getType().getName()
                            + "; valueType="
                            + (value == null
                                    ? "null"
                                    : value.getClass().getName()));

                } catch (Throwable e) {
                    log("Field: " + field.getName()
                            + "; access failed: "
                            + e.getClass().getSimpleName());
                }
            }

            type = type.getSuperclass();
        }

        try {
            Field candidateField = mc.getClass().getDeclaredField("bo");
            candidateField.setAccessible(true);
            Object candidate = candidateField.get(mc);

            log("===== Minecraft field bo diagnostics =====");
            log("Field bo value: " +
                    (candidate == null ? "null"
                            : candidate.getClass().getName()));

            if (candidate != null) {
                logCoordinateDiagnostics(candidate);
            }

            log("===== End Minecraft field bo diagnostics =====");
        } catch (Throwable e) {
            log("Field bo diagnostic failed: "
                    + e.getClass().getSimpleName()
                    + ": " + e.getMessage());
        }

        log("===== End Minecraft field diagnostics =====");
    }

    private static boolean hasCoordinateGetters(Class<?> type) {
        return hasNoArgMethod(type, "getX")
                && hasNoArgMethod(type, "getY")
                && hasNoArgMethod(type, "getZ");
    }

    private static boolean hasNoArgMethod(Class<?> type, String name) {
        try {
            Method method = type.getMethod(name);
            return method.getParameterCount() == 0
                    && (method.getReturnType() == double.class
                            || method.getReturnType() == Double.class);
        } catch (NoSuchMethodException e) {
            return false;
        }
    }

    private static boolean isRecognizablePlayerType(Class<?> type) {
        String name = type.getName();
        String simpleName = type.getSimpleName();

        return name.contains("LocalPlayer")
                || name.contains("ClientPlayerEntity")
                || simpleName.equals("LocalPlayer")
                || simpleName.equals("ClientPlayerEntity");
    }

    private static void logCoordinateDiagnostics(Object player) {
        log("===== FULL PLAYER CLASS DIAGNOSTICS =====");
        Class<?> runtimeClass = player.getClass();
        log("Runtime class: " + runtimeClass.getName());

        Class<?> type = runtimeClass;
        while (type != null) {
            log("--- CLASS " + type.getName() + " ---");

            for (Field field : type.getDeclaredFields()) {
                try {
                    field.setAccessible(true);
                    Object value = field.get(player);
                    String shown;

                    if (value == null) {
                        shown = "null";
                    } else if (field.getType().isPrimitive()
                            || value instanceof String
                            || value instanceof Number
                            || value instanceof Boolean
                            || value instanceof Character) {
                        shown = String.valueOf(value);
                    } else {
                        shown = "<" + value.getClass().getName() + ">";
                    }

                    log("FIELD " + field.getName()
                            + " type=" + field.getType().getName()
                            + " value=" + shown);
                } catch (Throwable e) {
                    log("FIELD " + field.getName()
                            + " access=" + e.getClass().getSimpleName());
                }
            }

            for (Method method : type.getDeclaredMethods()) {
                StringBuilder signature = new StringBuilder();
                signature.append("METHOD ")
                        .append(method.getName()).append("(");

                Class<?>[] params = method.getParameterTypes();
                for (int i = 0; i < params.length; i++) {
                    if (i > 0)
                        signature.append(",");
                    signature.append(params[i].getName());
                }

                signature.append(") -> ")
                        .append(method.getReturnType().getName());

                log(signature.toString());
            }

            type = type.getSuperclass();
        }

        log("===== END FULL PLAYER CLASS DIAGNOSTICS =====");
    }

    private static Double invokeCoordinate(Object player, String methodName) {
        if (player == null || methodName == null) {
            return null;
        }

        // First try normal, unobfuscated method names.
        try {
            java.lang.reflect.Method method = player.getClass().getMethod(methodName);

            if (method.getParameterCount() == 0) {
                Object value = method.invoke(player);
                if (value instanceof Number) {
                    return ((Number) value).doubleValue();
                }
            }
        } catch (Throwable ignored) {
            // Fall through to the Minecraft 1.21.4 obfuscated accessors.
        }

        // Minecraft 1.21.4 runtime mappings established from javap bytecode:
        // X -> dA(), Y -> dC(), Z -> dG()
        String obfuscatedName;

        switch (methodName) {
            case "getX":
                obfuscatedName = "dA";
                break;
            case "getY":
                obfuscatedName = "dC";
                break;
            case "getZ":
                obfuscatedName = "dG";
                break;
            default:
                return null;
        }

        try {
            java.lang.reflect.Method method = player.getClass().getMethod(obfuscatedName);

            Object value = method.invoke(player);
            if (value instanceof Number) {
                return ((Number) value).doubleValue();
            }
        } catch (Throwable ignored) {
            // Continue with superclass lookup below.
        }

        // The method may be declared in a superclass rather than the
        // runtime player's concrete class.
        Class<?> type = player.getClass().getSuperclass();

        while (type != null) {
            try {
                java.lang.reflect.Method method = type.getDeclaredMethod(obfuscatedName);

                method.setAccessible(true);

                Object value = method.invoke(player);
                if (value instanceof Number) {
                    return ((Number) value).doubleValue();
                }
            } catch (Throwable ignored) {
                // Try the next superclass.
            }

            type = type.getSuperclass();
        }

        return null;
    }
}