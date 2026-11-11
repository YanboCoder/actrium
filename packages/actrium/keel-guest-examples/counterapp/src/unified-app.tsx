import { Text, View } from 'keel/guest';

export default function CounterApp() {
  return (
    <View
      style={{
        flex: 1,
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: '#ffffff',
      }}
    >
      <Text style={{ fontSize: 32, fontWeight: '600', color: '#1a1a1a' }}>
        Counter
      </Text>
    </View>
  );
}
