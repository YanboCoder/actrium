import { useCallback, useState } from 'react';
import { ScrollView, Text, TextInput, View } from 'keel/guest';

type Message = {
  id: number;
  role: 'assistant' | 'user';
  text: string;
};

const INITIAL_MESSAGES: Message[] = [
  {
    id: 1,
    role: 'assistant',
    text: "Hi, I'm the Actrium assistant. How can I help?",
  },
  {
    id: 2,
    role: 'user',
    text: 'Take a look at my schedule for today.',
  },
  {
    id: 3,
    role: 'assistant',
    text: 'You have a product review this afternoon; the rest of the day is free.',
  },
  {
    id: 4,
    role: 'user',
    text: 'Help me organize the review materials.',
  },
  {
    id: 5,
    role: 'assistant',
    text: 'Done. The outline covers goals, scope, risks, and acceptance criteria.',
  },
  {
    id: 6,
    role: 'user',
    text: 'Add a timeline as well.',
  },
  {
    id: 7,
    role: 'assistant',
    text: 'I suggest 30 minutes for the review, 15 for discussion, and 15 for conclusions.',
  },
  {
    id: 8,
    role: 'user',
    text: 'Sounds good. Also remind me ten minutes before.',
  },
  {
    id: 9,
    role: 'assistant',
    text: "Noted. I'll remind you ten minutes before the review.",
  },
  {
    id: 10,
    role: 'user',
    text: 'Add a competitor comparison to the review materials.',
  },
  {
    id: 11,
    role: 'assistant',
    text: "Sure. I'll compare competitors across features, pricing, and ecosystem.",
  },
  {
    id: 12,
    role: 'user',
    text: 'Focus on the mobile experience.',
  },
  {
    id: 13,
    role: 'assistant',
    text: 'Mobile gets its own page covering launch speed, input experience, and offline capability.',
  },
  {
    id: 14,
    role: 'user',
    text: 'List the risks too.',
  },
  {
    id: 15,
    role: 'assistant',
    text: 'Risks include scope creep, API changes, performance regressions, and insufficient test coverage.',
  },
  {
    id: 16,
    role: 'user',
    text: 'Give each risk a mitigation.',
  },
  {
    id: 17,
    role: 'assistant',
    text: 'Added: scope freeze, API versioning, performance baselines, and automated regression.',
  },
  {
    id: 18,
    role: 'user',
    text: 'Okay, that is all for now.',
  },
]

export default function DialogApp() {
  const [messages, setMessages] = useState<Message[]>(INITIAL_MESSAGES);
  const [input, setInput] = useState('');

  const send = useCallback(() => {
    const text = input.trim();
    if (!text) return;

    setMessages((prev) => [
      ...prev,
      { id: Date.now(), role: 'user', text },
    ]);
    setInput('');
  }, [input]);

  return (
    <View style={{ flex: 1, backgroundColor: '#f5f7fa' }}>
      <View
        style={{
          paddingHorizontal: 20,
          paddingVertical: 14,
          backgroundColor: '#ffffff',
          borderBottomWidth: 1,
          borderBottomColor: '#e5e7eb',
        }}
      >
        <Text style={{ fontSize: 18, fontWeight: '600', color: '#111827' }}>
          Actrium Chat
        </Text>
      </View>

      <ScrollView
        style={{ flex: 1 }}
        contentContainerStyle={{ padding: 16 }}
        keyboardShouldPersistTaps="handled"
      >
        {messages.map((message) => {
          const isUser = message.role === 'user';
          return (
            <View
              key={message.id}
              style={{
                alignSelf: isUser ? 'flex-end' : 'flex-start',
                maxWidth: '80%',
                marginBottom: 10,
                paddingHorizontal: 12,
                paddingVertical: 10,
                borderRadius: 12,
                backgroundColor: isUser ? '#2563eb' : '#ffffff',
                borderWidth: isUser ? 0 : 1,
                borderColor: '#e5e7eb',
              }}
            >
              <Text
                style={{
                  fontSize: 15,
                  lineHeight: 21,
                  color: isUser ? '#ffffff' : '#1f2937',
                }}
              >
                {message.text}
              </Text>
            </View>
          );
        })}
      </ScrollView>

      <View
        style={{
          paddingHorizontal: 16,
          paddingTop: 8,
          paddingBottom: 14,
          backgroundColor: '#f5f7fa',
        }}
      >
        <View
          style={{
            width: '100%',
            minHeight: 96,
            backgroundColor: '#ffffff',
            borderWidth: 1,
            borderColor: '#d1d5db',
            borderRadius: 18,
            paddingHorizontal: 16,
            paddingTop: 12,
            paddingBottom: 52,
            overflow: 'hidden',
            shadowColor: '#000000',
            shadowOffset: { width: 0, height: 2 },
            shadowOpacity: 0.08,
            shadowRadius: 8,
          }}
        >
          <TextInput
            value={input}
            onChangeText={setInput}
            placeholder="Type a message..."
            placeholderTextColor="#9ca3af"
            multiline
            numberOfLines={2}
            returnKeyType="send"
            onSubmitEditing={send}
            style={{
              width: '100%',
              minHeight: 42,
              padding: 0,
              paddingRight: 0,
              backgroundColor: 'transparent',
              color: '#111827',
              fontSize: 15,
              lineHeight: 21,
            }}
          />
          <Text
            onPress={input.trim() ? send : undefined}
            style={{
              position: 'absolute',
              right: 12,
              bottom: 12,
              width: 30,
              height: 30,
              lineHeight: 30,
              borderRadius: 15,
              backgroundColor: input.trim() ? '#2563eb' : '#cbd5e1',
              color: '#ffffff',
              fontSize: 18,
              fontWeight: '600',
              textAlign: 'center',
            }}
          >
            ↑
          </Text>
        </View>
      </View>
    </View>
  );
}
