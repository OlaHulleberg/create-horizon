// SPDX-License-Identifier: MIT
import java.io.*;
import com.simibubi.create.content.kinetics.NetworkOracle;

public class NetworkBatch {
    public static void main(String[] args) throws Exception {
        try (var in = new BufferedReader(new InputStreamReader(System.in))) {
            for (String line; (line = in.readLine()) != null;)
                NetworkOracle.main(line.split(" "));
        }
    }
}
